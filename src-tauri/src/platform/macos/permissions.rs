use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::NSString;

use super::ffi::{AXIsProcessTrusted, AXIsProcessTrustedWithOptions};
use crate::platform::MicStatus;

pub fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Shows the system prompt that leads to the Accessibility settings.
pub fn request_accessibility() {
    let key = CFString::from_static_string("AXTrustedCheckOptionPrompt");
    let options =
        CFDictionary::from_CFType_pairs(&[(key.as_CFType(), CFBoolean::true_value().as_CFType())]);
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef().cast()) };
}

pub fn open_accessibility_settings() {
    open_settings("Privacy_Accessibility");
}

pub fn open_microphone_settings() {
    open_settings("Privacy_Microphone");
}

fn open_settings(anchor: &str) {
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
    if let Err(e) = std::process::Command::new("open").arg(url).spawn() {
        log::error!("could not open System Settings: {e}");
    }
}

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {
    static AVMediaTypeAudio: &'static NSString;
}

pub fn microphone_status() -> MicStatus {
    let Some(class) = AnyClass::get(c"AVCaptureDevice") else {
        return MicStatus::Unknown;
    };
    // AVAuthorizationStatus: 0 not determined, 1 restricted, 2 denied, 3 authorized.
    let status: isize = unsafe {
        let media: &NSString = AVMediaTypeAudio;
        let media: &AnyObject = media.as_ref();
        msg_send![class, authorizationStatusForMediaType: media]
    };
    match status {
        0 => MicStatus::NotDetermined,
        1 | 2 => MicStatus::Denied,
        3 => MicStatus::Granted,
        _ => MicStatus::Unknown,
    }
}
