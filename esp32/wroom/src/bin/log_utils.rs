#[macro_export]
macro_rules! logln {
    ($($arg:tt)*) => {{
        let up = embassy_time::Instant::now().as_secs();
        if let Some(utc) = $crate::log_utils::current_utc_string() {
            esp_println::println!("[up={}s {}] {}", up, utc, format_args!($($arg)*));
        } else {
            esp_println::println!("[up={}s unsynced] {}", up, format_args!($($arg)*));
        }
    }};
}
use core::cell::RefCell;

use critical_section::Mutex;
use embassy_time::Instant;

use crate::time_utils::unix_to_utc_string;

#[derive(Clone, Copy)]
struct TimeSyncState {
    base_unix_sec: Option<u64>,
    base_up_sec: u64,
}

static TIME_SYNC_STATE: Mutex<RefCell<TimeSyncState>> = Mutex::new(RefCell::new(TimeSyncState {
    base_unix_sec: None,
    base_up_sec: 0,
}));

pub fn set_time_sync_from_unix(unix_sec: u64) {
    let up_now = Instant::now().as_secs();
    critical_section::with(|cs| {
        let mut state = TIME_SYNC_STATE.borrow_ref_mut(cs);
        state.base_unix_sec = Some(unix_sec);
        state.base_up_sec = up_now;
    });
}

pub fn current_unix_time() -> Option<u64> {
    let up_now = Instant::now().as_secs();
    critical_section::with(|cs| {
        let state = TIME_SYNC_STATE.borrow_ref(cs);
        state
            .base_unix_sec
            .map(|base| base.saturating_add(up_now.saturating_sub(state.base_up_sec)))
    })
}

pub fn current_utc_string() -> Option<alloc::string::String> {
    current_unix_time().map(unix_to_utc_string)
}
