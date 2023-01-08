use std::{
    fs::File,
    io::stdout,
    path::{Path, PathBuf},
};

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ropey::Rope;

#[derive(Debug, Parser)]
#[clap(name = env!("CARGO_PKG_NAME"), version, author, about)]
struct Opts {
    path: PathBuf,
}

#[derive(Debug, Default)]
struct Buffer {
    rope: Rope,
}

impl Buffer {
    fn save(&self, path: &Path) -> Result<()> {
        self.rope.write_to(File::create(path)?)?;
        Ok(())
    }

    fn load(&mut self, path: &Path) -> Result<()> {
        self.rope = Rope::from_reader(File::open(path)?)?;
        Ok(())
    }
}

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
    let opts = Opts::parse();
    let path = &opts.path;

    let mut buffer = Buffer::default();
    if path.exists() {
        buffer.load(path)?;
    }

    Screen::init()?;
    loop {
        #[allow(clippy::single_match)]
        #[allow(clippy::collapsible_match)]
        match event::read()? {
            Event::Key(event) => match event {
                KeyEvent {
                    modifiers: KeyModifiers::CONTROL,
                    code,
                    ..
                } => match code {
                    KeyCode::Char('c') => {
                        break;
                    }
                    KeyCode::Char('s') => {
                        buffer.save(path)?;
                    }
                    _ => {}
                },
                _ => {}
            },
            _ => {}
        }
    }
    Screen::fini()?;
    Ok(())
}

#[cfg(test)]
include!("test.rs");
