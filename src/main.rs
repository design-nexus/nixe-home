mod builder;
mod credentials;
mod desktop_ui;
mod ha;
mod ha_automation;
mod prefs;
mod theme;
mod window;

use gtk::prelude::*;

const APP_ID: &str = "io.github.design_nexus.nixe.Home";

fn main() -> gtk::glib::ExitCode {
    nixe_core::app::init(nixe_core::app::App {
        slug: "nixe-home",
        name: "Home",
    });
    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_activate(window::present);
    app.connect_shutdown(|_| prefs::flush());
    app.run()
}
