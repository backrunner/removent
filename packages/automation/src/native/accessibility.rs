use super::*;
use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::{CFRange, CFType, CFTypeRef, TCFType},
    string::{CFString, CFStringRef},
};
use removent_proto::automation::TextPlacement;
use std::{
    collections::{HashMap, VecDeque},
    ffi::c_void,
};
mod applications;
pub use applications::apps;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        name: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementCopyActionNames(element: CFTypeRef, value: *mut CFArrayRef) -> i32;
    fn AXUIElementPerformAction(element: CFTypeRef, name: CFStringRef) -> i32;
    fn AXUIElementSetAttributeValue(element: CFTypeRef, name: CFStringRef, value: CFTypeRef)
    -> i32;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, seconds: f32) -> i32;
    fn AXUIElementGetTypeID() -> usize;
    fn AXValueGetTypeID() -> usize;
    fn AXValueGetType(value: CFTypeRef) -> u32;
    fn AXValueGetValue(value: CFTypeRef, kind: u32, output: *mut c_void) -> bool;
    fn AXValueCreate(kind: u32, value: *const c_void) -> CFTypeRef;
}
fn ax_result(code: i32) -> Result<(), Failure> {
    if code == 0 {
        Ok(())
    } else {
        Err(Failure::new(
            match code {
                -25204 => "outcome_unknown",
                -25202 => "stale_reference",
                -25211 => "permission_denied",
                _ => "accessibility_error",
            },
            format!(
                "Accessibility API returned {code}; refresh the application snapshot before continuing"
            ),
        ))
    }
}
fn attr(element: &CFType, name: &str) -> Option<CFType> {
    let name = CFString::new(name);
    let mut value = std::ptr::null();
    let code = unsafe {
        AXUIElementCopyAttributeValue(
            element.as_CFTypeRef(),
            name.as_concrete_TypeRef(),
            &mut value,
        )
    };
    if code != 0 || value.is_null() {
        None
    } else {
        Some(unsafe { CFType::wrap_under_create_rule(value) })
    }
}
fn string(value: &CFType) -> Option<String> {
    value
        .downcast::<CFString>()
        .filter(|s| s.char_len() <= 1024 * 1024)
        .map(|s| s.to_string())
}
fn text(element: &CFType, name: &str) -> Option<String> {
    attr(element, name)
        .as_ref()
        .and_then(string)
        .map(|s| s.chars().take(2048).collect())
}
fn actions(element: &CFType) -> Vec<String> {
    let mut value = std::ptr::null();
    if unsafe { AXUIElementCopyActionNames(element.as_CFTypeRef(), &mut value) } != 0
        || value.is_null()
    {
        return vec![];
    }
    let array = unsafe { CFArray::<CFType>::wrap_under_create_rule(value) };
    array.iter().take(32).filter_map(|v| string(&v)).collect()
}
fn set(element: &CFType, name: &str, value: CFTypeRef) -> Result<(), Failure> {
    let name = CFString::new(name);
    ax_result(unsafe {
        AXUIElementSetAttributeValue(element.as_CFTypeRef(), name.as_concrete_TypeRef(), value)
    })
}
fn secure(element: &CFType) -> bool {
    text(element, "AXSubrole").as_deref() == Some("AXSecureTextField")
}

fn boolean(element: &CFType, name: &str) -> Option<bool> {
    attr(element, name)?
        .downcast::<core_foundation::boolean::CFBoolean>()
        .map(|v| v.into())
}
fn geometry(element: &CFType, name: &str, kind: u32) -> Option<[f64; 2]> {
    let value = attr(element, name)?;
    if value.type_of() != unsafe { AXValueGetTypeID() }
        || unsafe { AXValueGetType(value.as_CFTypeRef()) } != kind
    {
        return None;
    }
    let mut output = [0f64; 2];
    if unsafe { AXValueGetValue(value.as_CFTypeRef(), kind, output.as_mut_ptr().cast()) } {
        Some(output)
    } else {
        None
    }
}
pub struct Accessibility {
    scope: String,
    generation: u64,
    refs: HashMap<String, Reference>,
}
impl Default for Accessibility {
    fn default() -> Self {
        Self {
            scope: format!("{:032x}", rand::random::<u128>()),
            generation: 0,
            refs: Default::default(),
        }
    }
}
struct Reference {
    element: CFType,
    process: applications::Identity,
    created: Instant,
}
impl Accessibility {
    fn reference(&self, id: &str) -> Result<&CFType, Failure> {
        let entry = self
            .refs
            .get(id)
            .filter(|r| r.created.elapsed() < Duration::from_secs(60) && r.process.alive())
            .ok_or_else(|| {
                Failure::new(
                    "stale_reference",
                    "Refresh the application's accessibility snapshot",
                )
            })?;
        // The old object may have been detached without the process terminating.
        if attr(&entry.element, "AXRole").is_none() {
            return Err(Failure::new("stale_reference", "Element no longer exists"));
        }
        Ok(&entry.element)
    }
    pub fn snapshot(&mut self, target: &str, cancel: &CancellationToken) -> Outcome {
        let process = applications::resolve(target)?;
        let root = unsafe { AXUIElementCreateApplication(process.pid) };
        if root.is_null() {
            return Err(failure("Cannot create application accessibility root"));
        }
        let root = unsafe { CFType::wrap_under_create_rule(root) };
        unsafe {
            AXUIElementSetMessagingTimeout(root.as_CFTypeRef(), 0.15);
        }
        if attr(&root, "AXRole").is_none() {
            return Err(Failure::new(
                "accessibility_unavailable",
                "Application does not expose an accessible tree",
            ));
        }
        self.generation += 1;
        self.refs.clear();
        let mut queue = VecDeque::from([(root, None, 0)]);
        let mut nodes = vec![];
        let mut truncated = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while let Some((element, parent, depth)) = queue.pop_front() {
            check_cancel(cancel)?;
            if nodes.len() >= 256 || Instant::now() >= deadline {
                queue.push_front((element, parent, depth));
                break;
            }
            unsafe {
                AXUIElementSetMessagingTimeout(element.as_CFTypeRef(), 0.15);
            }
            let id = format!("ax:{}:{}:{}", self.scope, self.generation, nodes.len());
            let protected = secure(&element);
            let value = if protected {
                None
            } else {
                text(&element, "AXValue")
            };
            nodes.push(json!({"reference":id,"parent":parent,"role":text(&element,"AXRole"),"subrole":text(&element,"AXSubrole"),
                "identifier":text(&element,"AXIdentifier"),"title":text(&element,"AXTitle"),"description":text(&element,"AXDescription"),"value":value,"protected":protected,"enabled":boolean(&element,"AXEnabled"),"focused":boolean(&element,"AXFocused"),"position_points":geometry(&element,"AXPosition",1),"size_points":geometry(&element,"AXSize",2),"actions":actions(&element)}));
            if depth < 20
                && let Some(children) = attr(&element, "AXChildren").and_then(|v| {
                    v.downcast::<CFArray>().map(|a| unsafe {
                        CFArray::<CFType>::wrap_under_get_rule(a.as_concrete_TypeRef())
                    })
                })
            {
                let capacity = 256 - queue.len().min(256);
                truncated |= children.len() as usize > capacity;
                for child in children.iter().take(capacity) {
                    // AX trees can contain self-links and repeated objects. Compare
                    // CF identities, not reference pointers, before allocating a node.
                    if child.type_of() == unsafe { AXUIElementGetTypeID() }
                        && *child != element
                        && !self.refs.values().any(|entry| entry.element == *child)
                        && !queue.iter().any(|(queued, _, _)| queued == &*child)
                    {
                        queue.push_back(((*child).clone(), Some(id.clone()), depth + 1));
                    }
                }
            }
            if depth >= 20 {
                truncated = true;
            }
            self.refs.insert(
                id,
                Reference {
                    element,
                    process,
                    created: Instant::now(),
                },
            );
        }
        Ok(
            json!({"target":format!("app:{}",process.pid),"generation":self.generation,"nodes":nodes,"truncated":truncated || !queue.is_empty(),"references_expire_seconds":60}),
        )
    }
    pub fn action(&self, id: &str, name: &str) -> Outcome {
        let element = self.reference(id)?;
        if !actions(element).iter().any(|n| n == name) {
            return Err(Failure::new(
                "unsupported_action",
                "Use an action advertised by the element",
            ));
        }
        let name = CFString::new(name);
        ax_result(unsafe {
            AXUIElementPerformAction(element.as_CFTypeRef(), name.as_concrete_TypeRef())
        })?;
        executed()
    }
    pub fn focus(&self, id: &str) -> Outcome {
        let element = self.reference(id)?;
        set(
            element,
            "AXFocused",
            core_foundation::boolean::CFBoolean::true_value().as_CFTypeRef(),
        )?;
        executed()
    }
    pub fn set_value(&self, id: &str, value: &str) -> Outcome {
        let element = self.reference(id)?;
        set(element, "AXValue", CFString::new(value).as_CFTypeRef())?;
        executed()
    }
    pub fn select(
        &self,
        id: &str,
        needle: &str,
        prefix: Option<&str>,
        suffix: Option<&str>,
        placement: TextPlacement,
    ) -> Outcome {
        let element = self.reference(id)?;
        if secure(element) {
            return Err(Failure::new(
                "protected_content",
                "Cannot inspect protected text",
            ));
        }
        let value = attr(element, "AXValue")
            .as_ref()
            .and_then(string)
            .filter(|s| s.len() <= 1024 * 1024)
            .ok_or_else(|| {
                Failure::new("unsupported_action", "Element has no bounded text value")
            })?;
        let (start, end) = text_range(&value, needle, prefix, suffix, placement)?;
        let range = CFRange {
            location: start as isize,
            length: (end - start) as isize,
        };
        let raw = unsafe { AXValueCreate(4, (&range as *const CFRange).cast()) };
        if raw.is_null() {
            return Err(failure("Cannot create text range"));
        }
        let range = unsafe { CFType::wrap_under_create_rule(raw) };
        set(element, "AXSelectedTextRange", range.as_CFTypeRef())?;
        executed()
    }
}
fn text_range(
    value: &str,
    needle: &str,
    prefix: Option<&str>,
    suffix: Option<&str>,
    placement: TextPlacement,
) -> Result<(usize, usize), Failure> {
    let matches: Vec<_> = value
        .char_indices()
        .filter(|(start, _)| value[*start..].starts_with(needle))
        .map(|(start, _)| start)
        .filter(|&start| {
            prefix.is_none_or(|p| value[..start].ends_with(p))
                && suffix.is_none_or(|s| value[start + needle.len()..].starts_with(s))
        })
        .take(2)
        .collect();
    if matches.len() != 1 {
        return Err(Failure::new(
            "ambiguous_text",
            "Text must match exactly once; supply prefix or suffix to disambiguate",
        ));
    }
    let start = value[..matches[0]].encode_utf16().count();
    let end = start + needle.encode_utf16().count();
    Ok(match placement {
        TextPlacement::Select => (start, end),
        TextPlacement::Before => (start, start),
        TextPlacement::After => (end, end),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_uses_utf16_and_rejects_ambiguous_matches() {
        assert_eq!(ax_result(-25204).unwrap_err().code, "outcome_unknown");
        assert_eq!(ax_result(-25202).unwrap_err().code, "stale_reference");
        assert_eq!(
            text_range("a😀中b", "中", None, None, TextPlacement::Select).unwrap(),
            (3, 4)
        );
        assert!(text_range("aaaa", "aa", None, None, TextPlacement::Select).is_err());
        assert_eq!(
            text_range("x one y one", "one", Some("y "), None, TextPlacement::After).unwrap(),
            (11, 11)
        );
    }
}
