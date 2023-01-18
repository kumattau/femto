#![allow(dead_code)]

use core::fmt;
use std::{
    io::{stdout, Write},
    ops::Range,
    path::{Path, PathBuf},
};

use anyhow::Result;
use clap::Parser;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    style::Print,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use euclid::{Point2D, Size2D, UnknownUnit};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

type Size = Size2D<usize, UnknownUnit>;
type Point = Point2D<usize, UnknownUnit>;

#[derive(clap::Parser)]
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

#[derive(PartialEq, Copy, Clone)]
enum Action {
    Resize(Size),
    Up,
    Down,
    Left,
    Right,
}

struct LineBr {
    data: Vec<u8>,
    span: Vec<(u8, u8)>,
}

impl LineBr {
    fn span(&self) -> impl Iterator<Item = (Range<usize>, Range<usize>)> + '_ {
        self.span.iter().scan((0..0, 0..0), |item, next| {
            item.0 = item.0.end..item.0.end + next.0 as usize;
            item.1 = item.1.end..item.1.end + next.1 as usize;
            Some(item.clone())
        })
    }
}

struct Buffer {
    line: Vec<LineBr>,
}

impl Buffer {
    fn rows(&self) -> usize {
        self.line.len()
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Buffer::from("")
    }
}

impl From<&str> for Buffer {
    fn from(text: &str) -> Self {
        let mut line: Vec<LineBr> = Default::default();
        let mut last;
        line.push(LineBr {
            data: vec![],
            span: vec![],
        });
        last = unsafe { line.last_mut().unwrap_unchecked() };
        for s in text.graphemes(true) {
            last.data.extend(s.as_bytes());
            last.span.push((s.len() as _, s.width() as _));
            if is_linebreak(s) {
                line.push(LineBr {
                    data: vec![],
                    span: vec![],
                });
                last = unsafe { line.last_mut().unwrap_unchecked() };
            }
        }
        last.span.push((0, 0));
        Self { line }
    }
}

impl fmt::Display for Buffer {
    fn fmt(&self, file: &mut fmt::Formatter<'_>) -> fmt::Result {
        for linebr in &self.line {
            write!(file, "{}", unsafe {
                std::str::from_utf8_unchecked(&linebr.data)
            })?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct Editor {
    buffer: Buffer,
    screen: Size,
    output: Vec<u8>,
    cursor: Point,
    offset: usize,
}

impl Editor {
    fn load(&mut self, path: &Path) -> Result<()> {
        self.buffer = Buffer::from(std::fs::read_to_string(path)?.as_str());
        Ok(())
    }
    fn save(&self, path: &Path) -> Result<()> {
        write!(std::fs::File::create(path)?, "{}", &self.buffer)?;
        Ok(())
    }
    fn exec(&mut self, action: Vec<Action>) -> Result<()> {
        if action.is_empty() {
            return Ok(());
        }
        let action = action[0];

        let mut full = false;

        if let Action::Resize(screen) = action {
            if self.screen != screen {
                self.screen = screen;
                full = true;
            }
        }

        let screen = self.screen;
        let buffer = &self.buffer;

        let mut offset = self.offset;
        let mut cursor = self.cursor;
        match action {
            Action::Up => {
                if 0 < cursor.y {
                    cursor.y -= 1;
                }
            }
            Action::Down => {
                if cursor.y + 1 < buffer.rows() {
                    cursor.y += 1;
                }
            }
            _ => {}
        }

        offset = offset.min(cursor.y);
        if cursor.y + 1 >= screen.height {
            offset = offset.max(cursor.y + 1 - screen.height);
        }

        let (marker, cursor) = 'outer: loop {
            let mut p = Point::new(0, 0);
            'inner: for (y, line) in buffer.line.iter().enumerate().skip(offset) {
                let mut n = false;
                let mut q = p;
                let mut r = 0;
                let mut s = r;
                for (x, (_, char)) in line.span().enumerate() {
                    if n && p != q {
                        break 'outer (Point::new(p.x, p.y), Point::new(r, y));
                    }
                    if cursor.y == y && (r..r + char.len()).contains(&cursor.x) {
                        match action {
                            Action::Left => break 'outer (Point::new(q.x, q.y), Point::new(s, y)),
                            Action::Right => n = true,
                            _ => break 'outer (Point::new(p.x, p.y), Point::new(r, y)),
                        }
                    }
                    if x + 1 == line.span.len() {
                        break;
                    }
                    if p != q {
                        q = p;
                        s = r;
                    }
                    r += char.len();
                    p.x += char.len();
                    if p.x >= screen.width {
                        p.x = 0;
                        p.y += 1;
                        if p.y >= screen.height {
                            break 'inner;
                        }
                    }
                }
                if cursor.y == y {
                    match action {
                        Action::Left => break 'outer (Point::new(q.x, q.y), Point::new(s, y)),
                        _ => break 'outer (Point::new(p.x, p.y), Point::new(cursor.x, y)),
                    }
                }
                p.x = 0;
                p.y += 1;
                if p.y >= screen.height {
                    break;
                }
            }
            offset += 1;
            if offset > buffer.line.len() {
                panic!();
            }
        };

        if self.offset != offset {
            full = true;
        }

        if full {
            self.output.clear();
            let s = self.screen;
            let mut p = Point::new(0, 0);
            for line in self.buffer.line.iter().skip(offset) {
                let mut eol = 0;
                for (x, (byte, char)) in line.span().enumerate() {
                    if x + 1 == line.span.len() {
                        break;
                    }
                    p.x += char.len();
                    if p.x >= s.width {
                        p.x = 0;
                        p.y += 1;
                        if p.y >= s.height {
                            break;
                        }
                    }
                    eol = byte.end;
                }
                self.output.extend(&line.data[..eol]);
                p.x = 0;
                p.y += 1;
                if p.y >= s.height {
                    break;
                }
                self.output.extend(b"\r\n");
            }
        }

        self.offset = offset;
        self.cursor = cursor;

        if full {
            execute!(
                stdout(),
                Hide,
                MoveTo(0, 0),
                Clear(ClearType::All),
                Print(unsafe { std::str::from_utf8_unchecked(&self.output) }),
                MoveTo(marker.x as _, marker.y as _),
                Show,
            )?;
        } else {
            execute!(stdout(), MoveTo(marker.x as _, marker.y as _),)?;
        }

        Ok(())
    }
}

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

    fn size() -> Result<Size> {
        let (cols, rows) = crossterm::terminal::size()?;
        Ok(Size::new(cols as _, rows as _))
    }
}

pub fn main() -> Result<()> {
    let opts = Opts::parse();
    let path = &opts.path;

    let mut editor = Editor::default();

    if path.exists() {
        editor.load(path)?;
    }
    Screen::init()?;
    editor.exec(vec![Action::Resize(Screen::size().unwrap())])?;
    loop {
        let mut action = vec![];
        match event::read()? {
            Event::Key(event) => match event {
                KeyEvent {
                    modifiers: KeyModifiers::CONTROL,
                    code,
                    ..
                } => match code {
                    KeyCode::Char('c') => break,
                    KeyCode::Char('s') => editor.save(path)?,
                    _ => {}
                },
                KeyEvent {
                    modifiers: KeyModifiers::NONE,
                    code,
                    ..
                } => match code {
                    KeyCode::Up => action.push(Action::Up),
                    KeyCode::Down => action.push(Action::Down),
                    KeyCode::Left => action.push(Action::Left),
                    KeyCode::Right => action.push(Action::Right),
                    _ => {}
                },
                _ => {}
            },
            Event::Resize(cols, rows) => {
                action.push(Action::Resize(Size::new(cols as _, rows as _)))
            }
            _ => {}
        }
        editor.exec(action)?;
    }
    Screen::fini()?;
    Ok(())
}

include!("test.rs");
