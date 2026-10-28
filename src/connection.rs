//! Connecting to Home Assistant: restoring the saved connection, the connect
//! dialog, the live link, and disconnecting.
use crate::{
    app::{self, Link},
    ha, prefs, rt,
};
use gtk::{glib, prelude::*};
use nyx_home::credentials::{self, LoadError};
use nyx_ui::widgets;
use std::{cell::RefCell, time::Duration};

/// Long enough to type a keyring password into the unlock prompt.
const KEYRING_WAIT: Duration = Duration::from_secs(120);

thread_local! {
    /// The running link task; aborting it ends the live stream.
    static LIVE: RefCell<Option<tokio::task::AbortHandle>> = const { RefCell::new(None) };
    /// Bumped on every connect so messages from an older link are ignored.
    static GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

enum Message {
    Ha(Box<ha::Update>),
    Rejected,
    Unreachable(String),
}

/// At startup: load the saved token and connect.
pub fn restore() {
    let Some(app) = app::get() else { return };
    if app.home.borrow().base.is_empty() {
        app::set_link(Link::NotSetUp);
        return;
    }
    app::set_link(Link::Unlocking);
    rt::blocking(
        || credentials::load(KEYRING_WAIT),
        |result| {
            let Some(app) = app::get() else { return };
            match result {
                Ok(token) => {
                    let base = app.home.borrow().base.clone();
                    connect(base, token);
                }
                Err(LoadError::Missing) => app::set_link(Link::NeedsToken),
                Err(LoadError::Unavailable(message)) => {
                    app::set_link(Link::Locked(message));
                    app::toast_action(
                        "Your keyring is locked, so Home can't read its token.",
                        "Try again",
                        restore,
                    );
                }
            }
        },
    );
}

pub fn reconnect() {
    let Some(app) = app::get() else { return };
    let (base, token) = {
        let home = app.home.borrow();
        (home.base.clone(), home.token.clone())
    };
    if token.is_empty() {
        restore();
    } else {
        connect(base, token);
    }
}

pub fn connect(base: String, token: String) {
    let Some(app) = app::get() else { return };
    stop();
    {
        let mut home = app.home.borrow_mut();
        home.base = base.clone();
        home.token = token.clone();
        home.link = Link::Connecting;
    }
    app::rebuild();
    let generation = GENERATION.with(|g| {
        g.set(g.get() + 1);
        g.get()
    });
    let (tx, rx) = async_channel::unbounded::<Message>();
    let task = rt::runtime().spawn(async move {
        let mut delay = 2;
        let inventory = loop {
            match ha::check(&base, &token).await {
                Err(ha::CheckError::Rejected) => {
                    let _ = tx.send(Message::Rejected).await;
                    return;
                }
                Err(error) => {
                    if tx
                        .send(Message::Unreachable(error.to_string()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Ok(_) => match ha::fetch_inventory(&base, &token).await {
                    Ok(inventory) => break inventory,
                    Err(error) => {
                        if tx.send(Message::Unreachable(error)).await.is_err() {
                            return;
                        }
                    }
                },
            }
            tokio::time::sleep(Duration::from_secs(delay)).await;
            delay = (delay * 2).min(60);
        };
        let metadata = inventory.metadata.clone();
        if tx
            .send(Message::Ha(Box::new(ha::Update::Connected(inventory))))
            .await
            .is_err()
        {
            return;
        }
        let (updates_tx, updates_rx) = async_channel::unbounded();
        let forward = {
            let tx = tx.clone();
            async move {
                while let Ok(update) = updates_rx.recv().await {
                    if tx.send(Message::Ha(Box::new(update))).await.is_err() {
                        return;
                    }
                }
            }
        };
        tokio::join!(ha::follow(base, token, updates_tx, metadata), forward);
    });
    LIVE.with(|live| *live.borrow_mut() = Some(task.abort_handle()));
    glib::spawn_future_local(async move {
        while let Ok(message) = rx.recv().await {
            if GENERATION.with(|g| g.get()) != generation {
                return;
            }
            receive(message);
        }
    });
}

fn receive(message: Message) {
    let Some(app) = app::get() else { return };
    let update = match message {
        Message::Ha(update) => *update,
        Message::Rejected => {
            app::set_link(Link::Rejected);
            app::toast_action(
                "Home Assistant rejected the saved token.",
                "Replace token",
                dialog,
            );
            return;
        }
        Message::Unreachable(error) => {
            let loaded = app.home.borrow().loaded();
            app::set_link(if loaded {
                Link::Offline(error)
            } else {
                Link::Unreachable(error)
            });
            return;
        }
    };
    match update {
        ha::Update::Connected(inventory) => {
            {
                let mut home = app.home.borrow_mut();
                home.areas = inventory.areas;
                home.device_count = inventory.device_count;
                home.devices = inventory.devices;
                home.info = inventory.info;
                home.entities = inventory
                    .entities
                    .into_iter()
                    .map(|e| (e.entity_id.clone(), e))
                    .collect();
            }
            app::rebuild();
        }
        ha::Update::Online => {
            if app.home.borrow().link != Link::Online {
                app::set_link(Link::Online);
            }
        }
        ha::Update::Offline(error) => app::set_link(Link::Offline(error)),
        ha::Update::Entity(entity) => {
            let id = entity.entity_id.clone();
            let known = app
                .home
                .borrow_mut()
                .entities
                .insert(id.clone(), entity)
                .is_some();
            if known {
                app::entity_changed(&id);
            } else {
                app::rebuild();
            }
        }
        ha::Update::Removed(id) => {
            app.home.borrow_mut().entities.remove(&id);
            app::rebuild();
        }
    }
}

fn stop() {
    GENERATION.with(|g| g.set(g.get() + 1));
    if let Some(task) = LIVE.with(|live| live.borrow_mut().take()) {
        task.abort();
    }
}

/// Forget the server and its token.
pub fn disconnect() {
    let Some(app) = app::get() else { return };
    stop();
    {
        let mut home = app.home.borrow_mut();
        home.base.clear();
        home.token.clear();
        home.clear();
        home.link = Link::NotSetUp;
    }
    prefs::update(|p| p.home_url.clear());
    app::rebuild();
    rt::blocking(
        || credentials::delete(Duration::from_secs(20)),
        |result| match result {
            Ok(()) => app::toast("Disconnected. The token was removed from your keyring."),
            Err(error) => app::toast(&format!("Disconnected, but {}", error.to_lowercase())),
        },
    );
}

fn valid_url(text: &str) -> Option<String> {
    let base = text.trim().trim_end_matches('/');
    let url = url::Url::parse(base).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none())
    .then(|| base.to_string())
}

/// The connect dialog: an address and a token, checked with Home Assistant
/// before anything is saved.
pub fn dialog() {
    let Some(app) = app::get() else { return };
    let (saved_base, saved_token) = {
        let home = app.home.borrow();
        (home.base.clone(), home.token.clone())
    };
    let (window, card) = widgets::dialog("Connect to Home Assistant", 480);
    let intro = widgets::label(
        "Home reads your devices from Home Assistant and keeps the access token in your desktop keyring.",
        "dim",
    );
    intro.set_wrap(true);
    card.append(&intro);

    let url_label = widgets::label("Server address", "field-label");
    let url = gtk::Entry::new();
    url.set_placeholder_text(Some("http://homeassistant.local:8123"));
    url.set_text(&saved_base);
    url.set_input_purpose(gtk::InputPurpose::Url);
    url.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
        url_label.upcast_ref()
    ])]);
    card.append(&url_label);
    card.append(&url);

    let token_label = widgets::label("Long-lived access token", "field-label");
    let token = gtk::PasswordEntry::new();
    token.set_show_peek_icon(true);
    token.set_placeholder_text(Some(if saved_token.is_empty() {
        "Paste the token here"
    } else {
        "Saved · leave empty to keep it"
    }));
    token.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
        token_label.upcast_ref()
    ])]);
    card.append(&token_label);
    card.append(&token);
    let hint = widgets::label(
        if saved_token.is_empty() {
            "In Home Assistant, open your profile, then Security, and create a long-lived access token."
        } else {
            "A token is saved in your keyring. Paste a new one only to replace it."
        },
        "dim",
    );
    hint.set_wrap(true);
    card.append(&hint);
    let create = gtk::Button::with_label("Create a token in Home Assistant…");
    create.add_css_class("flat");
    create.set_halign(gtk::Align::Start);
    {
        let url = url.clone();
        let window = window.clone();
        create.connect_clicked(move |_| match valid_url(&url.text()) {
            Some(base) => {
                gtk::UriLauncher::new(&format!("{base}/profile/security")).launch(
                    Some(&window),
                    None::<&gtk::gio::Cancellable>,
                    |_| {},
                );
            }
            None => url.add_css_class("error"),
        });
    }
    card.append(&create);

    let error = widgets::label("", "error-text");
    error.set_wrap(true);
    error.set_visible(false);
    error.set_accessible_role(gtk::AccessibleRole::Alert);
    card.append(&error);

    let buttons = widgets::hbox(8);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(6);
    let cancel = gtk::Button::with_label("Cancel");
    let connect_button = gtk::Button::with_label("Connect");
    connect_button.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&connect_button);
    card.append(&buttons);
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }

    let submit = {
        let (window, url, token, error, button) = (
            window.clone(),
            url.clone(),
            token.clone(),
            error.clone(),
            connect_button.clone(),
        );
        move || {
            let show_error = |message: &str| {
                error.set_text(message);
                error.set_visible(true);
            };
            url.remove_css_class("error");
            let Some(base) = valid_url(&url.text()) else {
                url.add_css_class("error");
                show_error(
                    "Enter the address you open Home Assistant with, starting with http:// or https://.",
                );
                url.grab_focus();
                return;
            };
            let entered = token.text().trim().to_string();
            let reuse = entered.is_empty() && !saved_token.is_empty();
            let secret = if reuse { saved_token.clone() } else { entered };
            if secret.is_empty() {
                show_error("Paste a long-lived access token.");
                token.grab_focus();
                return;
            }
            error.set_visible(false);
            button.set_sensitive(false);
            button.set_label("Checking…");
            let (window, error, button) = (window.clone(), error.clone(), button.clone());
            let checked_base = base.clone();
            let checked_secret = secret.clone();
            rt::spawn(
                async move { ha::check(&checked_base, &checked_secret).await },
                move |result| match result {
                    Err(problem) => {
                        error.set_text(&problem.to_string());
                        error.set_visible(true);
                        button.set_sensitive(true);
                        button.set_label("Connect");
                    }
                    Ok(info) => {
                        let stored = secret.clone();
                        let finish = move || {
                            prefs::update(|p| p.home_url = base.clone());
                            connect(base.clone(), secret.clone());
                            window.close();
                            app::toast(&if info.location_name.is_empty() {
                                "Connected to Home Assistant.".to_string()
                            } else {
                                format!("Connected to {}.", info.location_name)
                            });
                        };
                        if reuse {
                            finish();
                            return;
                        }
                        button.set_label("Saving…");
                        rt::blocking(
                            move || credentials::store(&stored, KEYRING_WAIT),
                            move |saved| match saved {
                                Ok(()) => finish(),
                                Err(message) => {
                                    error.set_text(&message);
                                    error.set_visible(true);
                                    button.set_sensitive(true);
                                    button.set_label("Connect");
                                }
                            },
                        );
                    }
                },
            );
        }
    };
    let submit = std::rc::Rc::new(submit);
    for entry in [url.upcast_ref::<gtk::Widget>(), token.upcast_ref()] {
        let submit = submit.clone();
        if let Some(entry) = entry.downcast_ref::<gtk::Entry>() {
            entry.connect_activate(move |_| submit());
        } else if let Some(entry) = entry.downcast_ref::<gtk::PasswordEntry>() {
            entry.connect_activate(move |_| submit());
        }
    }
    connect_button.connect_clicked(move |_| submit());
    window.present();
    if saved_base.is_empty() {
        url.grab_focus();
    } else {
        token.grab_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::valid_url;

    #[test]
    fn addresses_must_be_plain_http_urls() {
        assert_eq!(
            valid_url(" http://10.0.10.113:8123/ ").as_deref(),
            Some("http://10.0.10.113:8123")
        );
        assert_eq!(valid_url("10.0.10.113:8123"), None);
        assert_eq!(valid_url("http://user:pw@ha.local"), None);
        assert_eq!(valid_url("ftp://ha.local"), None);
    }
}
