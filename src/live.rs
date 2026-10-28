//! Live camera video: Home Assistant's HLS stream of a camera, played with
//! libmpv (nyx-playback) into a paintable. One stream plays at a time; a new
//! one takes over from the last, whose owner hears about it. Battery cameras
//! drain while streaming, so callers stop as soon as nobody is watching.
use crate::{app, ha};
use gtk::glib;
use nyx_playback::{
    mpv::{Event, Mpv},
    video,
};
use std::{cell::RefCell, rc::Rc};

/// Told why the stream ended when it wasn't stopped on purpose.
type Ended = Rc<dyn Fn(String)>;

struct Player {
    mpv: Mpv,
    /// The camera playing now, and who to tell if it ends or is taken over.
    current: Option<(String, Ended)>,
}

thread_local! {
    static PLAYER: RefCell<Option<Player>> = const { RefCell::new(None) };
}

/// The frames of whatever is playing.
pub fn paintable() -> video::VideoPaintable {
    video::paintable()
}

/// Start libmpv and its renderer on the main window, once.
fn ensure() -> Result<(), String> {
    if PLAYER.with(|p| p.borrow().is_some()) {
        return Ok(());
    }
    let app = app::get().ok_or("Home isn't running.")?;
    let (mpv, wakeup) = Mpv::new(&[
        ("vo", "libmpv"),
        ("hwdec", "auto-safe"),
        ("idle", "yes"),
        ("config", "no"),
        ("terminal", "no"),
        ("input-default-bindings", "no"),
        ("profile", "low-latency"),
        ("mute", "yes"),
        ("network-timeout", "30"),
    ])
    .map_err(|e| format!("Live video needs mpv's library (libmpv): {e}"))?;
    video::init(&app.window, &mpv, || {
        PLAYER
            .with(|p| {
                let p = p.try_borrow().ok()?;
                let m = &p.as_ref()?.mpv;
                Some((m.get_i64("dwidth")? as i32, m.get_i64("dheight")? as i32))
            })
            .unwrap_or((1280, 720))
    })
    .map_err(|e| format!("Live video can't draw here: {e}"))?;
    PLAYER.with(|p| *p.borrow_mut() = Some(Player { mpv, current: None }));
    // Drain mpv's events; an end we didn't ask for goes to the owner.
    glib::spawn_future_local(async move {
        while wakeup.recv().await.is_ok() {
            let mut ended: Option<(Ended, String)> = None;
            PLAYER.with(|p| {
                let Ok(mut p) = p.try_borrow_mut() else {
                    return;
                };
                let Some(player) = p.as_mut() else { return };
                while let Some(event) = player.mpv.next_event() {
                    if let Event::EndFile { reason, error } = event {
                        use nyx_playback::mpv::EndReason;
                        let why = match reason {
                            EndReason::Error => error
                                .map(|_| "The stream stopped with an error.".to_string())
                                .unwrap_or_else(|| "The stream stopped.".into()),
                            EndReason::Eof => "The camera stopped streaming.".into(),
                            _ => continue,
                        };
                        if let Some((_, owner)) = player.current.take() {
                            ended = Some((owner, why));
                        }
                    }
                }
            });
            if let Some((owner, why)) = ended {
                owner(why);
            }
        }
    });
    Ok(())
}

/// Play `camera` live. `ended` hears if the stream ends or another camera
/// takes over. Errors reach `ended` too.
pub fn play(camera: &str, ended: impl Fn(String) + 'static) {
    let ended: Ended = Rc::new(ended);
    if let Err(e) = ensure() {
        ended(e);
        return;
    }
    // Whoever was watching hears that this camera took over.
    let previous = PLAYER.with(|p| {
        p.borrow_mut()
            .as_mut()
            .and_then(|p| p.current.replace((camera.to_string(), ended.clone())))
    });
    if let Some((other, owner)) = previous
        && other != camera
    {
        owner("Another camera's live video started.".into());
        app::toast("Stopped the other live camera.");
    }
    video::clear();
    let id = camera.to_string();
    app::ha_try(
        move |base, token| async move { ha::camera_stream(&base, &token, &id).await },
        move |result| {
            let still_ours = PLAYER.with(|p| {
                p.borrow()
                    .as_ref()
                    .and_then(|p| p.current.as_ref())
                    .is_some_and(|(c, o)| Rc::ptr_eq(o, &ended) && !c.is_empty())
            });
            if !still_ours {
                return;
            }
            let outcome = result.and_then(|url| {
                PLAYER.with(|p| {
                    p.borrow()
                        .as_ref()
                        .ok_or_else(|| "Live video isn't available.".to_string())
                        .and_then(|p| {
                            p.mpv
                                .command(&["loadfile", &url])
                                .map_err(|_| "The live stream couldn't be opened.".to_string())
                        })
                })
            });
            if let Err(e) = outcome {
                PLAYER.with(|p| {
                    if let Some(p) = p.borrow_mut().as_mut() {
                        p.current = None;
                    }
                });
                ended(e);
            }
        },
    );
}

/// Stop `camera`'s stream if it's the one playing.
pub fn stop(camera: &str) {
    PLAYER.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut()
            && p.current.as_ref().is_some_and(|(c, _)| c == camera)
        {
            p.current = None;
            let _ = p.mpv.command(&["stop"]);
        }
    });
    video::clear();
}

pub fn set_muted(muted: bool) {
    PLAYER.with(|p| {
        if let Some(p) = p.borrow().as_ref() {
            let _ = p.mpv.set_flag("mute", muted);
        }
    });
}

/// Tear down before the app exits: the renderer goes before the player.
pub fn shutdown() {
    if let Some(player) = PLAYER.with(|p| p.borrow_mut().take()) {
        video::shutdown();
        player.mpv.destroy_later();
    }
}
