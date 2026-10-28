//! Loads Home's stylesheet over the shared 2.0 sheet for every bundled theme
//! and prints anything GTK rejects. Needs a display but opens no window:
//! `cargo run --example check_css`.
use std::{cell::Cell, rc::Rc};

#[path = "../src/kind_colors.rs"]
mod kind_colors;

fn main() {
    gtk::init().expect("no display");
    let errors = Rc::new(Cell::new(0));
    for theme in nyx_theme::bundled() {
        let css = format!(
            "{}\n{}\n{}",
            nyx_theme::sheet_for_check("home-window", &theme, true),
            kind_colors::css(&theme.look.swatches, &theme.palette.accent),
            include_str!("../src/style.css")
        );
        let provider = gtk::CssProvider::new();
        let (id, count) = (theme.id.clone(), errors.clone());
        provider.connect_parsing_error(move |_, section, error| {
            count.set(count.get() + 1);
            eprintln!(
                "{id}: line {}: {error}",
                section.start_location().lines() + 1
            );
        });
        provider.load_from_string(&css);
    }
    println!("{} parse errors", errors.get());
    std::process::exit(if errors.get() == 0 { 0 } else { 1 });
}
