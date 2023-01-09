use std::io::stdout;

use anyhow::Result;
use crossterm::{
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};

#[derive(Debug, Default)]
struct Screen;

impl Screen {
    fn init() -> Result<()> {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            _ = Screen::fini();
            hook(info);
        }));
        terminal::enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen)?;
        Ok(())
    }

    fn fini() -> Result<()> {
        execute!(stdout(), LeaveAlternateScreen)?;
        terminal::disable_raw_mode()?;
        Ok(())
    }
}

fn main() -> Result<()> {
    Screen::init()?;
    Screen::fini()?;
    Ok(())
}

#[cfg(test)]
include!("test.rs");
