use std::{
    fs::File,
    io::{stdout, Write},
    path::{Path, PathBuf},
};

use anyhow::Result;
use clap::Parser;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    style::Print,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Parser)]
#[clap(name = env!("CARGO_PKG_NAME"), version, author, about)]
struct Opts {
    path: PathBuf,
}

fn is_linebreak(str: &str) -> bool {
    matches!(
        str,
        "\r\n"
            | "\u{000A}"
            | "\u{000B}"
            | "\u{000C}"
            | "\u{000D}"
            | "\u{0085}"
            | "\u{2028}"
            | "\u{2029}"
    )
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

    fn size() -> Result<(usize, usize)> {
        let (cols, rows) = terminal::size()?;
        Ok((cols as _, rows as _))
    }

    fn draw(buffer: &Buffer) -> Result<()> {
        let mut stdout = stdout();

        let (cols, rows) = Screen::size()?;

        queue!(stdout, cursor::Hide)?;
        queue!(stdout, terminal::Clear(terminal::ClearType::All))?;
        queue!(stdout, cursor::MoveTo(0, 0))?;

        let mut row = 0;
        'outer: for line in buffer.rope.lines() {
            let mut col = 0;
            for segm in line.to_string().graphemes(true) {
                if !is_linebreak(segm) {
                    col += segm.width();
                    if col >= cols {
                        col = 0;
                        row += 1;
                        if row >= rows {
                            break 'outer;
                        }
                    }
                    queue!(stdout, Print(segm))?;
                }
            }
            row += 1;
            if row >= rows {
                break 'outer;
            }
            queue!(stdout, Print("\r\n"))?;
        }

        queue!(stdout, cursor::Show)?;
        stdout.flush()?;
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
        Screen::draw(&buffer)?;

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
