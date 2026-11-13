//! Use the tray's GTK main thread on Linux; no external dialog executable.
#[cfg(target_os = "linux")]
fn show(title: &str, text: &str, question: bool) -> bool {
    use gtk::prelude::*;
    let dialog = gtk::MessageDialog::new(
        None::<&gtk::Window>,
        gtk::DialogFlags::MODAL,
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
    dialog.set_title(title);
    dialog.set_width_request(300);
    dialog.set_default_response(if question {
        gtk::ResponseType::No
    } else {
        gtk::ResponseType::Ok
    });
    let accepted = dialog.run() == gtk::ResponseType::Yes;
    dialog.close();
    accepted
}

#[cfg(windows)]
fn show(title: &str, text: &str, question: bool) -> bool {
    rfd::MessageDialog::new()
        .set_title(title)
        .set_description(text)
        .set_buttons(if question {
            rfd::MessageButtons::YesNo
        } else {
            rfd::MessageButtons::Ok
        })
        .show()
        == rfd::MessageDialogResult::Yes
}

pub fn alert(title: &str, text: &str) {
    show(title, text, false);
}

pub fn confirm(title: &str, text: &str) -> bool {
    show(title, text, true)
}
