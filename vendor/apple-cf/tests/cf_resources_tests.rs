use apple_cf::cf::{
    CFBundle, CFCalendar, CFCharacterSet, CFDate, CFDateFormatter, CFDateFormatterStyle,
    CFFileSecurity, CFLocale, CFNumber, CFNumberFormatter, CFNumberFormatterStyle, CFPreferences,
    CFString, CFTimeZone, CFURL, CFUUID, CFXML,
};
use apple_cf::raw;
use std::process;

#[test]
fn cf_resource_wrappers_work() {
    let bundle_url =
        CFURL::from_file_system_path("/System/Library/Frameworks/CoreFoundation.framework", true)
            .expect("bundle URL");
    let bundle = CFBundle::from_url(&bundle_url).expect("bundle");
    assert!(bundle.bundle_url().has_directory_path());

    let locale = CFLocale::new("en_US");
    let calendar = CFCalendar::new("gregorian").expect("gregorian calendar");
    let time_zone = CFTimeZone::new("GMT").expect("GMT time zone");
    calendar.set_time_zone(&time_zone);
    assert_eq!(locale.identifier().to_string(), "en_US");
    assert_eq!(calendar.time_zone().name().to_string(), "GMT");

    let charset = CFCharacterSet::from_characters_in_string(&CFString::new("abc"));
    assert!(charset.contains('a'));
    assert!(!charset.inverted().contains('a'));

    let number_formatter = CFNumberFormatter::new(Some(&locale), CFNumberFormatterStyle::Decimal);
    let rendered = number_formatter.format_number(&CFNumber::from_i64(1234));
    assert!(!rendered.is_empty());
    assert!(number_formatter
        .parse_number(&CFString::new("42"))
        .is_some());

    let date_formatter = CFDateFormatter::new(
        Some(&locale),
        CFDateFormatterStyle::Short,
        CFDateFormatterStyle::NoStyle,
    );
    assert!(!date_formatter.format_date(&CFDate::now()).is_empty());

    let app_id = CFString::new(&format!("com.doomfish.apple-cf.tests.{}", process::id()));
    let key = CFString::new("example-key");
    CFPreferences::set_app_value(&key, Some(&CFString::new("value")), &app_id);
    assert!(CFPreferences::synchronize(&app_id));
    assert!(CFPreferences::app_value(&key, &app_id).is_some());
    CFPreferences::set_app_value(&key, None, &app_id);
    let _ = CFPreferences::synchronize(&app_id);

    let file_security = CFFileSecurity::new();
    let owner = CFUUID::new();
    assert!(file_security.set_owner_uuid(&owner));
    assert!(file_security.owner_uuid().is_some());
    assert!(file_security.set_mode(0o644));
    assert_eq!(file_security.mode(), Some(0o644));

    let escaped = CFXML::escape_entities(&CFString::new("<tag>value</tag>"));
    assert_eq!(
        CFXML::unescape_entities(&escaped).to_string(),
        "<tag>value</tag>"
    );
}

#[test]
fn cf_url_constructors_reject_invalid_input() {
    assert!(CFURL::from_string("http://example.com/a b").is_none());
    assert!(CFURL::from_string("http://example.com/%zz").is_none());
    assert!(CFURL::from_string("http://example.com/\0").is_none());
    assert!(CFURL::from_file_system_path("", false).is_none());
    assert!(CFURL::from_file_system_path("/tmp/a\0b", false).is_none());
    let url = CFURL::from_string("https://example.com/a").expect("url");
    assert_eq!(url.absolute_string().to_string(), "https://example.com/a");
}

fn macos_major_version() -> u32 {
    let output = process::Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .expect("sw_vers");
    String::from_utf8(output.stdout)
        .expect("sw_vers output")
        .trim()
        .split('.')
        .next()
        .and_then(|major| major.parse().ok())
        .expect("macOS major version")
}

#[test]
fn cf_url_from_string_rejects_malformed_authorities_from_macos_27() {
    let rejects = macos_major_version() >= 27;
    for malformed in ["http://[bad", "http://bad]", "http://user@@host/"] {
        let parsed = CFURL::from_string(malformed).map(|url| url.absolute_string().to_string());
        if rejects {
            assert_eq!(parsed, None, "{malformed:?}");
        } else {
            assert_eq!(parsed.as_deref(), Some(malformed));
        }
    }
}

#[test]
fn cf_url_file_system_path_is_none_for_non_file_paths() {
    let invalid_utf8 = CFURL::from_string("file:///tmp/%FF").expect("url");
    assert!(invalid_utf8.file_system_path().is_none());
    let mailto = CFURL::from_string("mailto:someone@example.com").expect("url");
    assert!(mailto.file_system_path().is_none());
    let file = CFURL::from_file_system_path("/tmp/apple-cf", false).expect("url");
    assert_eq!(
        file.file_system_path().expect("path").to_string(),
        "/tmp/apple-cf"
    );
}

#[test]
fn cf_url_absolute_string_resolves_relative_urls() {
    let base = CFURL::from_string("http://example.com/a/").expect("base");
    let relative = CFString::new("b/c");
    let ptr = unsafe {
        raw::CFURLCreateWithString(
            std::ptr::null(),
            relative.as_ptr().cast(),
            base.as_ptr().cast(),
        )
    };
    let url = unsafe { CFURL::from_raw(ptr.cast_mut().cast()) }.expect("relative url");
    assert_eq!(url.absolute_string().to_string(), "http://example.com/a/b/c");
}

#[test]
fn locale_calendar_and_time_zone_constructors_handle_bad_names() {
    assert!(CFTimeZone::new("Bogus/Zone").is_none());
    assert!(CFTimeZone::new("GMT\0").is_none());
    assert!(CFCalendar::new("bogus").is_none());
    assert!(CFCalendar::new("gregorian\0").is_none());
    assert_eq!(CFLocale::new("en\0US").identifier().to_string(), "en");
}

#[test]
fn character_set_membership_covers_non_bmp_characters() {
    let charset = CFCharacterSet::from_characters_in_string(&CFString::new("a😀"));
    assert!(charset.contains('a'));
    assert!(charset.contains('😀'));
    assert!(!charset.contains('\u{F600}'));
    assert!(!charset.contains('😁'));
    assert!(charset.inverted().contains('😁'));
    assert!(!charset.inverted().contains('😀'));

    let planes = CFCharacterSet::from_characters_in_string(&CFString::new("é中b𠀀\u{10FFFF}"));
    for member in ['é', '中', 'b', '𠀀', '\u{10FFFF}'] {
        assert!(planes.contains(member), "{member:?}");
    }
    for non_member in ['\0', '\u{FFFF}', 'a'] {
        assert!(!planes.contains(non_member), "{non_member:?}");
    }
}

#[test]
fn character_set_from_unpaired_surrogates_holds_the_replacement_character() {
    let units = [0x61_u16, 0xD83D];
    let ptr = unsafe { raw::CFStringCreateWithCharacters(std::ptr::null(), units.as_ptr(), 2) };
    let string = unsafe { CFString::from_raw(ptr.cast_mut().cast()) }.expect("string");
    let charset = CFCharacterSet::from_characters_in_string(&string);
    assert!(charset.contains('a'));
    assert!(charset.contains('\u{FFFD}'));
    assert!(!charset.contains('😀'));
}

#[test]
fn file_security_rejects_out_of_range_modes() {
    let file_security = CFFileSecurity::new();
    assert!(!file_security.set_mode(0x1_0000));
    assert!(file_security.set_mode(0o600));
    assert_eq!(file_security.mode(), Some(0o600));
}

#[test]
fn bundle_resource_lookup_rejects_nul() {
    let url =
        CFURL::from_file_system_path("/System/Library/Frameworks/CoreFoundation.framework", true)
            .expect("bundle URL");
    let bundle = CFBundle::from_url(&url).expect("bundle");
    assert!(bundle.resource_url("Info\0", Some("plist"), None).is_none());
    assert!(bundle.resource_url("Info", Some("pl\0ist"), None).is_none());
}
