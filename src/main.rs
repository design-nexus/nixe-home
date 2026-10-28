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
    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_activate(app::present);
    app.connect_shutdown(|_| prefs::flush());
    app.run()
}
