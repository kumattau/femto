use std::{
    io::{stdout, Write},
    path::{Path, PathBuf},
};

use anyhow::Result;
use clap::Parser;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    style::Print,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
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

#[derive(Debug)]
enum Course {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Default)]
struct Cursor {
    col: usize,
    row: usize,
}

impl Cursor {
    fn jump(&mut self, buffer: &Buffer, course: Course) {
        match course {
            Course::Up => {
                if 0 < self.row {
                    self.row -= 1;
                    self.col = self.col.min(buffer.cols(self.row) - 1);
                }
            }
            Course::Down => {
                if self.row + 1 < buffer.rows() {
                    self.row += 1;
                    self.col = self.col.min(buffer.cols(self.row) - 1);
                }
            }
            Course::Left => {
                if 0 < self.col {
                    self.col -= 1;
                }
            }
            Course::Right => {
                if self.col + 1 < buffer.cols(self.row) {
                    self.col += 1;
                }
            }
        }
    }
}

#[derive(Debug, Default)]
struct LineBr {
    text: Vec<u8>,
    span: Vec<(u8, u8)>,
    cols: usize,
}

#[derive(Debug)]
struct Buffer {
    line: Vec<LineBr>,
}

impl Default for Buffer {
    fn default() -> Self {
        Self {
            line: vec![LineBr {
                text: vec![],
                span: vec![(0, 1)],
                cols: 1,
            }],
        }
    }
}

impl Buffer {
    fn load(&mut self, path: &Path) -> Result<()> {
        let text = std::fs::read_to_string(path)?;

        let mut line: Vec<LineBr> = Default::default();
        let mut last;
        line.push(Default::default());
        last = unsafe { line.last_mut().unwrap_unchecked() };
        for segm in text.graphemes(true) {
            last.text.extend(segm.as_bytes());
            if !is_linebreak(segm) {
                last.span.push((segm.len() as _, segm.width() as _));
                last.cols += segm.width();
            } else {
                last.span.push((segm.len() as _, 1));
                last.cols += 1;
                line.push(Default::default());
                last = unsafe { line.last_mut().unwrap_unchecked() };
            }
        }
        last.span.push((0, 1));
        last.cols += 1;
        self.line = line;
        Ok(())
    }

    fn save(&mut self, path: &Path) -> Result<()> {
        let mut file = std::fs::File::create(path)?;
        for line in &self.line {
            write!(file, "{}", unsafe {
                std::str::from_utf8_unchecked(&line.text)
            })?;
        }
        Ok(())
    }

    fn rows(&self) -> usize {
        self.line.len()
    }

    fn cols(&self, row: usize) -> usize {
        self.line[row].span.len()
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

    fn draw(buffer: &Buffer, cursor: &Cursor, offset: &mut usize) -> Result<()> {
        let (cols, rows) = Screen::size()?;

        let mut off = *offset;
        off = off.min(cursor.row);
        if cursor.row + 1 >= rows {
            off = off.max(cursor.row + 1 - rows);
        }

        let mut all = *offset == 0 || *offset != off;

        let mut cur = None;
        let mut buf = all
            .then(|| Vec::<u8>::with_capacity(cols * rows * 4))
            .unwrap_or_default();
        'outer: loop {
            buf.clear();
            let mut row = 0;
            for (j, lbr) in buffer.line.iter().enumerate().skip(off) {
                let mut col = 0;
                let mut end = 0;
                for (i, (len, wid)) in lbr.span.iter().enumerate() {
                    if cursor.col == i && cursor.row == j {
                        cur = Some(Cursor { col, row });
                        if !all {
                            break 'outer;
                        }
                    }
                    if i == lbr.span.len() - 1 {
                        break;
                    }
                    col += *wid as usize;
                    if col >= cols {
                        col = 0;
                        row += 1;
                        if row >= rows {
                            break;
                        }
                    }
                    end += *len as usize;
                }
                if all {
                    buf.extend(&lbr.text.as_slice()[..end]);
                }
                row += 1;
                if row >= rows {
                    break;
                }
                if all {
                    buf.extend(b"\r\n");
                }
            }
            if cur.is_some() {
                break;
            }
            off += 1;
            all = true;
        }
        let cur = unsafe { cur.unwrap_unchecked() };

        *offset = off;

        if all {
            execute!(
                stdout(),
                cursor::Hide,
                terminal::Clear(terminal::ClearType::All),
                cursor::MoveTo(0, 0),
                Print(unsafe { std::str::from_utf8_unchecked(&buf) }),
                cursor::MoveTo(cur.col as _, cur.row as _),
                cursor::Show
            )?;
        } else {
            execute!(stdout(), cursor::MoveTo(cur.col as _, cur.row as _),)?;
        }

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
    let mut cursor = Cursor::default();
    let mut offset = usize::default();

    Screen::init()?;
    loop {
        Screen::draw(&buffer, &cursor, &mut offset)?;

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
                KeyEvent {
                    modifiers: KeyModifiers::NONE,
                    code,
                    ..
                } => match code {
                    KeyCode::Up => {
                        cursor.jump(&buffer, Course::Up);
                    }
                    KeyCode::Down => {
                        cursor.jump(&buffer, Course::Down);
                    }
                    KeyCode::Left => {
                        cursor.jump(&buffer, Course::Left);
                    }
                    KeyCode::Right => {
                        cursor.jump(&buffer, Course::Right);
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
