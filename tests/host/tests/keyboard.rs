pub use wisp_host_tests::keymap;
pub use wisp_host_tests::line_editor;

#[allow(dead_code)]
mod sync {
    pub struct Locked<T>(std::sync::Mutex<T>);
    impl<T> Locked<T> {
        pub const fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
        pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
            f(&mut self.0.lock().unwrap())
        }
    }
}

#[allow(dead_code)]
mod usb_hid {
    pub fn poll() { panic!("hardware polling must not run in decoder tests"); }
}

#[allow(dead_code)]
#[path = "../../../src/keyboard.rs"]
mod keyboard;
