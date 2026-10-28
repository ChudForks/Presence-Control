//! Real Windows repaint regression, using an isolated window with no app data.
//! Damage the presented client area, invalidate it through Windows, then verify
//! the actual window DC. A Slint snapshot would repaint independently and mask
//! the defect this probe is intended to catch.
#[cfg(windows)]
mod probe {
    use slint::ComponentHandle;
    use std::{cell::RefCell, rc::Rc, time::Duration};
    use windows_sys::Win32::{
        Foundation::{HWND, RECT},
        Graphics::Gdi::{
            CreateSolidBrush, DeleteObject, FillRect, GdiFlush, GetDC, GetPixel, InvalidateRect,
            ReleaseDC,
        },
        UI::WindowsAndMessaging::{FindWindowW, GetClientRect},
    };

    slint::slint! {
        export component RepaintProbe inherits Window {
            in property <string> probe-title;
            title: probe-title;
            width: 480px;
            height: 320px;
            background: #132537;
            Rectangle { x: 20px; y: 20px; width: 40px; height: 20px; background: #87adcb; }
        }
    }

    struct Pending {
        points: Vec<(i32, i32, u32)>,
        label: String,
    }

    struct State {
        tick: usize,
        pending: Option<Pending>,
        checks: Vec<String>,
        outcome: Option<Result<(), String>>,
    }

    fn window(title: &[u16]) -> Result<HWND, String> {
        // SAFETY: the NUL-terminated title identifies only this probe's window.
        let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        if hwnd.is_null() {
            return Err("The isolated repaint window was not created.".into());
        }
        Ok(hwnd)
    }

    fn pixels(hwnd: HWND, coordinates: &[(i32, i32)]) -> Result<Vec<u32>, String> {
        // SAFETY: the DC is released after these synchronous read-only calls.
        unsafe {
            let dc = GetDC(hwnd);
            if dc.is_null() {
                return Err("Could not read the probe's client area.".into());
            }
            let values = coordinates
                .iter()
                .map(|&(x, y)| GetPixel(dc, x, y))
                .collect();
            ReleaseDC(hwnd, dc);
            Ok(values)
        }
    }

    fn damage(hwnd: HWND, scenario: usize) -> Result<Pending, String> {
        let mut client = RECT::default();
        if unsafe { GetClientRect(hwnd, &mut client) } == 0 {
            return Err("Could not read the probe's client rectangle.".into());
        }
        let (rect, label) = match scenario {
            0 => (
                RECT {
                    left: 91,
                    top: 83,
                    right: 173,
                    bottom: 141,
                },
                "Partial Windows invalidation repaints unchanged UI",
            ),
            1 => (
                RECT {
                    left: client.right / 2,
                    top: client.bottom / 2,
                    right: client.right - 1,
                    bottom: client.bottom - 1,
                },
                "Bottom-right Windows invalidation repaints unchanged UI",
            ),
            _ => (client, "Full Windows invalidation repaints unchanged UI"),
        };
        let mut coordinates = Vec::new();
        // Include the first and last damaged pixels to catch fractional-scale
        // rounding errors that leave a one-pixel stale fringe.
        for x in [rect.left, (rect.left + rect.right) / 2, rect.right - 1] {
            for y in [rect.top, (rect.top + rect.bottom) / 2, rect.bottom - 1] {
                coordinates.push((x, y));
            }
        }
        let before = pixels(hwnd, &coordinates)?;
        if before.contains(&0xffff_ffff) {
            return Err("Could not sample the visible probe window.".into());
        }
        // Simulate pixels discarded by Windows, independently of Slint's scene.
        // SAFETY: only this probe's client rectangle is painted; handles are
        // released before returning to its normal Slint event loop.
        unsafe {
            let dc = GetDC(hwnd);
            let brush = CreateSolidBrush(0xff00ff);
            if dc.is_null() || brush.is_null() {
                if !dc.is_null() {
                    ReleaseDC(hwnd, dc);
                }
                if !brush.is_null() {
                    DeleteObject(brush);
                }
                return Err("Could not create the repaint fault.".into());
            }
            FillRect(dc, &rect, brush);
            GdiFlush();
            DeleteObject(brush);
            ReleaseDC(hwnd, dc);
        }
        if pixels(hwnd, &coordinates)?
            .iter()
            .any(|&value| value != 0xff00ff)
        {
            return Err("The repaint fault did not reach the actual window.".into());
        }
        if unsafe { InvalidateRect(hwnd, &rect, 0) } == 0 {
            return Err("Could not invalidate the probe's damaged area.".into());
        }
        Ok(Pending {
            points: coordinates
                .into_iter()
                .zip(before)
                .map(|((x, y), p)| (x, y, p))
                .collect(),
            label: label.into(),
        })
    }

    pub fn run() -> Result<(), String> {
        let ui = RepaintProbe::new().map_err(|error| error.to_string())?;
        let title = format!(
            "Presence Control isolated repaint probe {}",
            std::process::id()
        );
        ui.set_probe_title(title.clone().into());
        let title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
        let state = Rc::new(RefCell::new(State {
            tick: 0,
            pending: None,
            checks: Vec::new(),
            outcome: None,
        }));
        let runner = state.clone();
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(250),
            move || {
                let mut state = runner.borrow_mut();
                if state.outcome.is_some() {
                    return;
                }
                let result = (|| {
                    // Allow initial presentation to settle before injecting damage.
                    if state.tick < 3 {
                        state.tick += 1;
                        return Ok(());
                    }
                    let hwnd = window(&title)?;
                    if let Some(pending) = state.pending.take() {
                        let coordinates: Vec<_> =
                            pending.points.iter().map(|&(x, y, _)| (x, y)).collect();
                        let after = pixels(hwnd, &coordinates)?;
                        if after
                            .iter()
                            .zip(&pending.points)
                            .any(|(&got, &(_, _, expected))| got != expected)
                        {
                            return Err(format!(
                                "{}: stale pixels remain in the presented window",
                                pending.label
                            ));
                        }
                        state.checks.push(pending.label);
                    }
                    if state.checks.len() < 3 {
                        state.pending = Some(damage(hwnd, state.checks.len())?);
                    } else {
                        state.outcome = Some(Ok(()));
                        slint::quit_event_loop().map_err(|error| error.to_string())?;
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    state.outcome = Some(Err(error));
                    let _ = slint::quit_event_loop();
                }
            },
        );
        ui.run().map_err(|error| error.to_string())?;
        timer.stop();
        let state = state.borrow();
        let outcome = state
            .outcome
            .clone()
            .unwrap_or_else(|| Err("Probe closed early.".into()));
        let report = serde_json::json!({
            "passed": outcome.is_ok(),
            "checks": state.checks,
            "error": outcome.as_ref().err(),
            "scaleFactor": ui.window().scale_factor(),
        });
        println!("{report}");
        outcome
    }
}

fn main() {
    #[cfg(windows)]
    if let Err(error) = probe::run() {
        eprintln!("Repaint regression: {error}");
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    eprintln!("The repaint probe requires Windows.");
}
