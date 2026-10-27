mod builder;
mod canvas;
mod credentials;
mod desktop_ui;
mod ha;
mod ha_automation;
mod home_features;
mod prefs;
mod theme;
mod window;

use gtk::prelude::*;

const APP_ID: &str = "io.github.design_nexus.nyx.Home";

fn main() -> gtk::glib::ExitCode {
    nyx_core::app::init(nyx_core::app::App {
        slug: "nyx-home",
        name: "Home",
    });
    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_activate(window::present);
    app.connect_shutdown(|_| prefs::flush());
    app.run()
}
