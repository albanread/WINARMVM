//! The RUSTTCL control channel — `macvm-gui` made drivable from a script.
//!
//! The Cocoa app has had this since CG5 (`cocoa_gui/src/control.rs`); the
//! cross-platform GUI had nothing, so on Windows the only way to see the
//! environment was for a human to look at it. That is false the moment the
//! screen is scriptable: `gui connect 7645`, `gui doit …`, `gui snap out.png`,
//! read the PNG. This is that channel, ported to the shell seam so it serves
//! both hosts from one implementation.
//!
//! Opt-in (an env var naming a port), **loopback only**: a listener thread
//! accepts one connection at a time from `macvm rusttcl`'s `gui` verb and
//! forwards each request to the UI thread through the host's existing wake
//! (`PostMessageW` on Windows, `performSelectorOnMainThread:` on macOS),
//! where it runs alongside any other main-thread work. The listener thread
//! never touches the window or the VM — it queues, wakes, and relays.
//!
//! **WINARM (WG1): this file is SHARED, not copied.** `win_gui`
//! (`macvm-winui`, the Smalltalk-authored native shell) includes this exact
//! source with `#[path]` rather than carrying a second listener, because two
//! implementations of one wire protocol drift and the drift shows up as a
//! script that works against one app and not the other. What WG1 changed to
//! make that possible is the whole of the difference: the env var, the log
//! prefix and the UI-thread wake are now PARAMETERS instead of
//! `MACVM_GUI_CTL`, `"macvm-gui"` and `crate::shell::waker()`. The protocol,
//! the framing, the `sleep`-answered-on-the-listener trick and the 20-second
//! reply bound are untouched. `macvm-gui` keeps `MACVM_GUI_CTL` and
//! `macvm-winui` takes `MACVM_WINUI_CTL`, so both stay independently
//! drivable in one session.
//!
//! Protocol (both directions): `<len>\n<len bytes>`, one request in flight per
//! connection — byte-identical to the Cocoa channel's, so the SAME rusttcl
//! `gui` verbs drive either app with no client-side branch.
//!
//! ```text
//!   ping            -> OK pong
//!   eval <st>       -> OK <printString> | ERR <error>
//!   doit <st>       -> OK | ERR <error>
//!   view <name>     -> OK                 (switch the visible view)
//!   snap <path>     -> OK | ERR <reason>  (client-area PNG)
//!   sleep <ms>      -> OK                 (listener-side pause, so a script
//!                                          can wait out an async render
//!                                          without a Tcl sleep verb)
//! ```
//!
//! **Why `snap` never blocks the UI thread.** WebView2's `CapturePreview` is
//! asynchronous, and P4 recorded the rule the hard way: calling
//! `wait_for_async_operation` from the UI thread runs a NESTED message loop,
//! which re-enters and can deadlock. So the UI thread only *starts* the
//! capture and returns to its loop immediately; the completion handler (also
//! on the UI thread, later) answers this request's reply channel. The
//! LISTENER thread is the one that blocks — which is safe, because it is a
//! plain worker with nothing else to do. Blocking the right thread is the
//! whole trick.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::time::Duration;

/// One queued request: the command line, and the channel the UI thread (or a
/// capture completion handler) answers on.
pub struct CtlReq {
    pub cmd: String,
    pub reply: SyncSender<String>,
}

/// Read one `<len>\n<bytes>` frame. `Ok(None)` is a clean disconnect.
fn read_frame(s: &mut TcpStream) -> std::io::Result<Option<String>> {
    let mut len_line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match s.read(&mut byte)? {
            0 => return Ok(None),
            _ if byte[0] == b'\n' => break,
            _ => {
                len_line.push(byte[0]);
                // A malformed peer must not make us allocate unboundedly.
                if len_line.len() > 16 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "control: length line too long",
                    ));
                }
            }
        }
    }
    let len: usize = String::from_utf8_lossy(&len_line)
        .trim()
        .parse()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "control: bad length"))?;
    if len > 1 << 20 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "control: frame too large",
        ));
    }
    let mut buf = vec![0u8; len];
    s.read_exact(&mut buf)?;
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

fn write_frame(s: &mut TcpStream, body: &str) -> std::io::Result<()> {
    write!(s, "{}\n", body.len())?;
    s.write_all(body.as_bytes())?;
    s.flush()
}

/// Start the listener if `env_var` names a port; answer the receiver the UI
/// thread drains. `None` when the channel is off, which is the default —
/// this is a debugging surface, not a feature, and it opens a socket.
///
/// `app` prefixes the diagnostics so a session running both hosts can tell
/// which one spoke. `wake` is the host's UI-thread poke, fired after every
/// queued request; it must be safe to call from a foreign thread and safe to
/// call before the window exists (`macvm-gui` reads its HWND at notify time
/// for exactly that reason; `macvm-winui` posts a THREAD message, which needs
/// no window at all).
pub fn start(
    env_var: &str,
    app: &str,
    wake: std::sync::Arc<dyn Fn() + Send + Sync>,
) -> Option<Receiver<CtlReq>> {
    let port: u16 = match std::env::var(env_var) {
        Ok(s) => match s.trim().parse() {
            Ok(p) => p,
            Err(_) => {
                eprintln!("{app}: {env_var}={s} is not a port — control channel off");
                return None;
            }
        },
        Err(_) => return None,
    };

    // Loopback only, deliberately: this evaluates arbitrary Smalltalk in the
    // running image. It is a local debugging channel and must never be
    // reachable off-box.
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{app}: control channel could not bind 127.0.0.1:{port}: {e}");
            return None;
        }
    };
    eprintln!("{app}: control channel on 127.0.0.1:{port} (rusttcl `gui connect {port}`)");

    let (tx, rx) = sync_channel::<CtlReq>(8);
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let _ = conn.set_nodelay(true);
            loop {
                let cmd = match read_frame(&mut conn) {
                    Ok(Some(c)) => c,
                    Ok(None) | Err(_) => break,
                };

                // `sleep` is answered HERE, not on the UI thread: its whole
                // purpose is to let a script wait for an async render (a page
                // load, a capture) to settle, so handing it to the UI thread
                // would block exactly the thread that has to do the work.
                if let Some(ms) = cmd.strip_prefix("sleep ") {
                    if let Ok(ms) = ms.trim().parse::<u64>() {
                        std::thread::sleep(Duration::from_millis(ms.min(30_000)));
                        let _ = write_frame(&mut conn, "OK");
                        continue;
                    }
                }

                let (rtx, rrx) = sync_channel::<String>(1);
                if tx
                    .send(CtlReq {
                        cmd: cmd.clone(),
                        reply: rtx,
                    })
                    .is_err()
                {
                    let _ = write_frame(&mut conn, "ERR gui is shutting down");
                    break;
                }
                wake();

                // Generous, but bounded: a capture of a large page can take a
                // moment, and a hung UI thread must not hang the script
                // forever — a timeout reply is diagnostic, a hang is not.
                let reply = rrx
                    .recv_timeout(Duration::from_secs(20))
                    .unwrap_or_else(|_| "ERR timeout waiting for the UI thread".to_string());
                if write_frame(&mut conn, &reply).is_err() {
                    break;
                }
            }
        }
    });
    Some(rx)
}
