//! Portal capability probe.
//!
//! Run: `cargo run -p flick-adapters-linux --example portal_probe`
//!
//! Probes the Portal GlobalShortcuts interface and reports capability status.
//! Does NOT call BindShortcuts (to avoid triggering user config UI).

fn main() {
    let diag = flick_adapters_linux::diagnostics::probe_session();

    println!("Session type: {:?}", diag.session_type);
    println!("WAYLAND_DISPLAY: {:?}", diag.wayland_display);
    println!("XDG_RUNTIME_DIR: {:?}", diag.runtime_dir);
    println!("DBUS_SESSION_BUS_ADDRESS: {:?}", diag.session_bus);
    println!("XDG_CURRENT_DESKTOP: {:?}", diag.current_desktop);
    println!("Portal GlobalShortcuts: {:?}", diag.portal_global_shortcuts);
    println!("Portal backend: {:?}", diag.portal_backend);

    match diag.session_type {
        flick_adapters_linux::diagnostics::SessionType::Wayland => {
            println!("\n✓ Wayland session detected. Portal probe can proceed.");
        }
        flick_adapters_linux::diagnostics::SessionType::X11 => {
            println!("\n✗ X11 session detected. Flick does not support X11.");
        }
        flick_adapters_linux::diagnostics::SessionType::Unknown => {
            println!("\n? Unknown session type. Cannot determine Portal availability.");
        }
    }
}
