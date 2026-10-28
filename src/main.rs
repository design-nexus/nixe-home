mod activity;
mod app;
mod automations;
mod builder;
mod canvas;
mod connection;
mod connections;
mod ha;
mod ha_automation;
mod home_features;
mod icons;
mod overview;
mod prefs;
mod present;
mod rooms;
mod rt;
mod service;
mod settings;
mod theme;
mod tiles;

use gtk::prelude::*;

const APP_ID: &str = "io.github.design_nexus.nyx.Home";

fn main() -> gtk::glib::ExitCode {
    nyx_core::app::init(nyx_core::app::App {
        slug: "nyx-home",
        name: "Home",
    });
    let mut builder = gtk::Application::builder().application_id(APP_ID);
    // The developer aids open a separate copy instead of handing over to a
    // Home that is already open.
    if std::env::var_os("NYX_HOME_PAGE").is_some() || std::env::var_os("NYX_HOME_OPEN").is_some() {
        builder = builder.flags(gtk::gio::ApplicationFlags::NON_UNIQUE);
    }
    let app = builder.build();
    app.connect_activate(app::present);
    app.connect_shutdown(|_| prefs::flush());
    app.run()
}
