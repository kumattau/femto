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

#[derive(Debug, PartialEq, Clone, Copy)]
enum Action {
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
    #[allow(dead_code)]
    fn jump(&mut self, buffer: &Buffer, action: Action) {
        match action {
            Action::Up => {
                if 0 < self.row {
                    self.row -= 1;
                    self.col = self.col.min(buffer.cols(self.row) - 1);
                }
            }
            Action::Down => {
                if self.row + 1 < buffer.rows() {
                    self.row += 1;
                    self.col = self.col.min(buffer.cols(self.row) - 1);
                }
            }
            Action::Left => {
                if 0 < self.col {
                    self.col -= 1;
                }
            }
            Action::Right => {
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
        self.line[row].cols
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
}

#[derive(Debug, Default)]
struct Editor {
    offset: usize,
    buffer: Buffer,
    cursor: Cursor,
}

impl Editor {
    fn draw(&mut self, action: &mut Option<Action>) -> Result<()> {
        let offset = &mut self.offset;
        let buffer = &mut self.buffer;
        let cursor = &mut self.cursor;

        let (cols, rows) = Screen::size()?;

        let mut cur_col = cursor.col;
        let mut cur_row = cursor.row;

        match action {
            Some(Action::Up) if 0 < cur_row => cur_row -= 1,
            Some(Action::Down) if cur_row + 1 < buffer.rows() => cur_row += 1,
            Some(Action::Right) | Some(Action::Left) => {
                cur_col = cur_col.min(buffer.cols(cur_row) - 1)
            }
            _ => {}
        }

        let mut off = *offset;
        off = off.min(cur_row);
        if cur_row + 1 >= rows {
            off = off.max(cur_row + 1 - rows);
        }

        let mut all = *offset == 0 || *offset != off;

        let mut cur = None;
        let mut buf = all
            .then(|| Vec::<u8>::with_capacity(cols * rows * 4))
            .unwrap_or_default();

        'outer: loop {
            buf.clear();
            let mut yet = true;
            let act = *action;

            let mut row = 0;
            for (lpt, lbr) in buffer.line.iter().enumerate().skip(off) {
                let mut col = 0;

                let mut ptr = 0;
                let mut bgn = 0;

                let mut col_pre = col;
                let mut row_pre = row;
                let mut bgn_pre = bgn;

                for (cpt, (len, wid)) in lbr.span.iter().enumerate() {
                    let end = bgn + *wid as usize;
                    if cur.is_none() && lpt == cur_row && (bgn..end).contains(&cur_col) {
                        match act {
                            Some(Action::Right) if yet && cur_col + 1 < buffer.cols(cur_row) => {
                                cur_col = end;
                                yet = false; // cur will be determined by the next iteration
                            }
                            Some(Action::Left) if 0 < cur_col => {
                                cur_col = if cur_col > bgn { bgn } else { bgn_pre };
                                cur = Some(Cursor {
                                    col: col_pre,
                                    row: row_pre,
                                });
                            }
                            _ => cur = Some(Cursor { col, row }),
                        }
                    }
                    if cur.is_some() && !all {
                        break 'outer;
                    }
                    if cpt == lbr.span.len() - 1 {
                        break;
                    }
                    col_pre = col;
                    col += *wid as usize;
                    if col >= cols {
                        col = 0;
                        row_pre = row;
                        row += 1;
                        if row >= rows {
                            break;
                        }
                    }
                    bgn_pre = bgn;
                    bgn = end;
                    ptr += *len as usize;
                }
                if cur.is_none()
                    && lpt == cur_row
                    && (Some(Action::Up) == act || Some(Action::Down) == act)
                {
                    cur = Some(Cursor { col, row });
                }
                if all {
                    buf.extend(&lbr.text.as_slice()[..ptr]);
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

        *offset = off;
        *action = None;
        cursor.col = cur_col;
        cursor.row = cur_row;

        Ok(())
    }
}

fn main() -> Result<()> {
    let opts = Opts::parse();
    let path = &opts.path;

    let mut editor = Editor::default();

    if path.exists() {
        editor.buffer.load(path)?;
    }
    let mut action = None;

    Screen::init()?;
    loop {
        editor.draw(&mut action)?;

        #[allow(clippy::single_match)]
        #[allow(clippy::collapsible_match)]
        match event::read()? {
            Event::Key(event) => match event {
                KeyEvent {
                    modifiers: KeyModifiers::CONTROL,
                    code,
                    ..
                } => match code {
                    KeyCode::Char('c') => break,
                    KeyCode::Char('s') => editor.buffer.save(path)?,
                    _ => {}
                },
                KeyEvent {
                    modifiers: KeyModifiers::NONE,
                    code,
                    ..
                } => match code {
                    KeyCode::Up => action = Some(Action::Up),
                    KeyCode::Down => action = Some(Action::Down),
                    KeyCode::Left => action = Some(Action::Left),
                    KeyCode::Right => action = Some(Action::Right),
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
