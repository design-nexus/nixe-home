mod activity;
mod app;
mod automations;
mod builder;
mod cameras;
mod canvas;
mod cards;
mod connection;
mod connections;
mod details;
mod device;
mod grid;
mod ha;
mod ha_automation;
mod home_features;
mod icons;
mod kind_colors;
mod light_groups;
mod live;
mod overview;
mod prefs;
mod present;
mod robot_map;
mod robot_window;
mod rooms;
mod rt;
mod service;
mod settings;
mod theme;
mod viewer;

use gtk::prelude::*;

const APP_ID: &str = "io.github.design_nexus.nyx.Home";

fn main() -> gtk::glib::ExitCode {
    nyx_core::app::init(nyx_core::app::App {
        slug: "nyx-home",
        name: "Home",
    });
    // Developer aid: NYX_HOME_TEST=1 gives a test copy its own ID, so its
    // windows can be kept off the screen (a compositor rule on the ID).
    let id = if std::env::var_os("NYX_HOME_TEST").is_some() {
        format!("{APP_ID}Test")
    } else {
        APP_ID.to_string()
    };
    let mut builder = gtk::Application::builder().application_id(id);
    // The developer aids open a separate copy instead of handing over to a
    // Home that is already open.
    if std::env::var_os("NYX_HOME_PAGE").is_some() || std::env::var_os("NYX_HOME_OPEN").is_some() {
        builder = builder.flags(gtk::gio::ApplicationFlags::NON_UNIQUE);
    }
    let app = builder.build();
    app.connect_activate(app::present);
    app.connect_shutdown(|_| {
        live::shutdown();
        prefs::flush();
    });
    app.run()
}
