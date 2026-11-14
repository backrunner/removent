//! Nonblocking native dialogs. Polling the tray must continue while one is visible.
#[cfg(target_os = "linux")]
pub struct Dialog {
    window: gtk::MessageDialog,
    answer: std::rc::Rc<std::cell::Cell<Option<bool>>>,
}
#[cfg(target_os = "linux")]
impl Dialog {
    pub fn new(title: &str, text: &str, question: bool) -> Self {
        use gtk::prelude::*;
        let window = gtk::MessageDialog::new(
            None::<&gtk::Window>,
            gtk::DialogFlags::empty(),
            if question {
                gtk::MessageType::Question
            } else {
                gtk::MessageType::Info
            },
            if question {
                gtk::ButtonsType::YesNo
            } else {
                gtk::ButtonsType::Ok
            },
            text,
        );
        window.set_title(title);
        window.set_width_request(300);
        window.set_default_response(if question {
            gtk::ResponseType::No
        } else {
            gtk::ResponseType::Ok
        });
        let answer = std::rc::Rc::new(std::cell::Cell::new(None));
        let result = answer.clone();
        window.connect_response(move |window, response| {
            if result.get().is_none() {
                result.set(Some(response == gtk::ResponseType::Yes));
            }
            // close() emits another DeleteEvent response and would overwrite
            // an affirmative answer. Hide now; release the window after poll.
            window.hide();
        });
        window.show_all();
        window.present();
        Self { window, answer }
    }
    pub fn poll(&self) -> Option<bool> {
        self.answer.get()
    }
    pub fn close(&self) {
        use gtk::prelude::*;
        self.window.response(gtk::ResponseType::No);
    }
}
#[cfg(target_os = "linux")]
impl Drop for Dialog {
    fn drop(&mut self) {
        use gtk::prelude::*;
        self.window.close();
    }
}

#[cfg(windows)]
pub struct Dialog {
    thread: std::sync::Arc<std::sync::atomic::AtomicU32>,
    answer: std::sync::mpsc::Receiver<bool>,
    cancelled: std::cell::Cell<bool>,
    question: bool,
}
#[cfg(windows)]
impl Dialog {
    pub fn new(title: &str, text: &str, question: bool) -> Self {
        use std::sync::{
            Arc,
            atomic::{AtomicU32, Ordering},
            mpsc,
        };
        use windows_sys::Win32::{
            System::Threading::GetCurrentThreadId, UI::WindowsAndMessaging::*,
        };
        let thread = Arc::new(AtomicU32::new(0));
        let id = thread.clone();
        let (send, answer) = mpsc::channel();
        let wide = |text: &str| {
            text.replace('\0', "")
                .encode_utf16()
                .chain(Some(0))
                .collect::<Vec<_>>()
        };
        let title = wide(title);
        let text = wide(text);
        std::thread::spawn(move || {
            id.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
            let buttons = if question {
                MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION
            } else {
                MB_OK | MB_ICONINFORMATION
            };
            let answer = unsafe {
                MessageBoxW(
                    std::ptr::null_mut(),
                    text.as_ptr(),
                    title.as_ptr(),
                    buttons | MB_SETFOREGROUND,
                )
            };
            id.store(0, Ordering::Release);
            let _ = send.send(answer == IDYES);
        });
        Self {
            thread,
            answer,
            cancelled: std::cell::Cell::new(false),
            question,
        }
    }
    pub fn poll(&self) -> Option<bool> {
        if self.cancelled.get() {
            self.close();
        }
        match self.answer.try_recv() {
            Ok(answer) => Some(answer),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(false),
            Err(_) => None,
        }
    }
    pub fn close(&self) {
        use windows_sys::Win32::{
            Foundation::{HWND, LPARAM},
            UI::WindowsAndMessaging::*,
        };
        unsafe extern "system" fn dismiss(window: HWND, button: LPARAM) -> windows_sys::core::BOOL {
            unsafe {
                let mut process = 0;
                GetWindowThreadProcessId(window, &mut process);
                if process == windows_sys::Win32::System::Threading::GetCurrentProcessId() {
                    PostMessageW(window, WM_COMMAND, button as usize, 0);
                }
            }
            1
        }
        self.cancelled.set(true);
        let thread = self.thread.load(std::sync::atomic::Ordering::Acquire);
        if thread != 0 {
            // Enumerate only this dialog's private worker thread. Repeating on
            // each poll also closes a window created just after cancellation.
            unsafe {
                EnumThreadWindows(
                    thread,
                    Some(dismiss),
                    if self.question { IDNO } else { IDOK } as isize,
                );
            }
        }
    }
}
#[cfg(windows)]
impl Drop for Dialog {
    fn drop(&mut self) {
        self.close();
    }
}
