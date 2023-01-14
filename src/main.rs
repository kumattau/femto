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
use euclid::{Point2D, Size2D};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

struct U; // dummy Unit for euclid

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
    Full,
    Up,
    Down,
    Left,
    Right,
    Resize(Size2D<usize, U>),
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

    fn size() -> Result<Size2D<usize, U>> {
        let (cols, rows) = terminal::size()?;
        Ok(Size2D::new(cols as _, rows as _))
    }
}

#[derive(Debug, Default)]
struct Editor {
    offset: usize,
    buffer: Buffer,
    colrow: Size2D<usize, U>,
    cursor: Point2D<usize, U>,
}

impl Editor {
    fn draw(&mut self, action: Action) -> Result<()> {
        let offset = &mut self.offset;
        let buffer = &mut self.buffer;
        let cursor = &mut self.cursor;

        let mut all = false;

        if action == Action::Full {
            all = true;
        }

        if let Action::Resize(colrow) = action {
            self.colrow = colrow;
            all = true;
        }
        let (cols, rows) = (self.colrow.width, self.colrow.height);

        let mut cur_col = cursor.x;
        let mut cur_row = cursor.y;

        match action {
            Action::Up if 0 < cur_row => cur_row -= 1,
            Action::Down if cur_row + 1 < buffer.rows() => cur_row += 1,
            Action::Right | Action::Left => cur_col = cur_col.min(buffer.cols(cur_row) - 1),
            _ => {}
        }

        let mut off = *offset;
        off = off.min(cur_row);
        if cur_row + 1 >= rows {
            off = off.max(cur_row + 1 - rows);
        }
        if *offset != off {
            all = true;
        }

        let mut cur: Option<Point2D<usize, U>> = None;
        let mut buf = all
            .then(|| Vec::<u8>::with_capacity(cols * rows * 4))
            .unwrap_or_default();

        'outer: loop {
            buf.clear();
            let mut yet = true;

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
                        match action {
                            Action::Right if yet && cur_col + 1 < buffer.cols(cur_row) => {
                                cur_col = end;
                                yet = false; // cur will be determined by the next iteration
                            }
                            Action::Left if 0 < cur_col => {
                                cur_col = if cur_col > bgn { bgn } else { bgn_pre };
                                cur = Some(Point2D::new(col_pre, row_pre));
                            }
                            _ => cur = Some(Point2D::new(col, row)),
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
                    && (Action::Up == action || Action::Down == action)
                {
                    cur = Some(Point2D::new(col, row));
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
                cursor::MoveTo(cur.x as _, cur.y as _),
                cursor::Show
            )?;
        } else {
            execute!(stdout(), cursor::MoveTo(cur.x as _, cur.y as _),)?;
        }

        *offset = off;
        cursor.x = cur_col;
        cursor.y = cur_row;

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
    Screen::init()?;
    editor.draw(Action::Resize(Screen::size().unwrap()))?;
    loop {
        #[allow(clippy::single_match)]
        #[allow(clippy::collapsible_match)]
        let action = match event::read()? {
            Event::Key(event) => match event {
                KeyEvent {
                    modifiers: KeyModifiers::CONTROL,
                    code,
                    ..
                } => match code {
                    KeyCode::Char('c') => break,
                    KeyCode::Char('s') => {
                        editor.buffer.save(path)?;
                        continue;
                    }
                    _ => continue,
                },
                KeyEvent {
                    modifiers: KeyModifiers::NONE,
                    code,
                    ..
                } => match code {
                    KeyCode::Up => Action::Up,
                    KeyCode::Down => Action::Down,
                    KeyCode::Left => Action::Left,
                    KeyCode::Right => Action::Right,
                    _ => Action::Full,
                },
                _ => continue,
            },
            Event::Resize(cols, rows) => {
                Action::Resize(Size2D::<usize, U>::new(cols as _, rows as _))
            }
            _ => continue,
        };
        editor.draw(action)?;
    }
    Screen::fini()?;
    Ok(())
}

#[cfg(test)]
include!("test.rs");
