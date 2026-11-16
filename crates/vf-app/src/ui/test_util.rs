//! UI test helpers shared by the tab tests.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

/// How many frames a test's `run` may take to settle. kittest's own limit is 4. A worker
/// thread (the document preview, a job) asks the window to repaint when it is done, at a
/// moment of its own, and under a full run that pushed a `run` to 5 or 6 frames now and then:
/// the test then failed with "exceeded max_steps" though nothing was wrong.
pub const MAX_STEPS: u64 = 40;
/// The processor time this thread has used so far (kernel and user). A speed test measures
/// the code with this, not with a clock: another program building on the same machine made
/// the clock say 110 and 159 ms for work that takes about 40, and the test failed for it.
pub fn thread_cpu() -> std::time::Duration {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
    let (mut made, mut ended, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: the pseudo-handle of this thread, and four FILETIMEs that outlive the call.
    let ok = unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut made,
            &mut ended,
            &mut kernel,
            &mut user,
        )
    };
    assert!(ok.is_ok(), "thread times not read");
    let ticks = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    // In 100-nanosecond units.
    std::time::Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
}

/// Clicks the button labelled `label`, then runs until the UI settles.
pub fn click<S>(h: &mut Harness<'_, S>, label: &str) {
    h.get_by_role_and_label(Role::Button, label).click();
    h.run();
}

/// Focuses the field labelled `label` and types `text` (appended to what is there).
pub fn type_into<S>(h: &mut Harness<'_, S>, label: &str, text: &str) {
    h.get_by_label(label).click();
    h.step();
    h.get_by_label(label).type_text(text);
    h.step();
}

/// Binds the process-wide pdfium before a test starts anything that looks for it. The vendored
/// `filer_pdf::pdf::find_pdfium` test-binds the library; if that overlaps the first
/// `filer_pdf::pdf::pdfium` call on another test thread, the "already initialized" error is
/// cached for the whole test process. The app binds once at startup, so only tests overlap.
pub fn pdfium_first() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if let Some(dir) = filer_pdf::pdf::find_pdfium(std::path::Path::new("")) {
            let _ = filer_pdf::pdf::pdfium(&dir);
        }
    });
}
