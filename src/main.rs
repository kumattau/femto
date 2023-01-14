use std::{
    io::{stdout, Write},
    ops::Range,
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

struct U;
type Size = Size2D<usize, U>;
type Point = Point2D<usize, U>;

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
    Resize(Size),
}

#[derive(Debug, Default)]
struct LineBr {
    text: Vec<u8>,
    span: Vec<(u8, u8)>,
    cols: usize,
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

    fn size() -> Result<Size> {
        let (cols, rows) = terminal::size()?;
        Ok(Size::new(cols as _, rows as _))
    }
}

#[derive(Debug, Default)]
struct Editor {
    offset: usize,
    buffer: Buffer,
    scrsiz: Size,
    cursor: Point,
}

impl Editor {
    fn draw(&mut self, action: Action) -> Result<()> {
        let mut all = false;

        if action == Action::Full {
            all = true;
        }

        if let Action::Resize(scrsiz) = action {
            self.scrsiz = scrsiz;
            all = true;
        }

        match action {
            Action::Up if 0 < self.cursor.y => self.cursor.y -= 1,
            Action::Down if self.cursor.y + 1 < self.buffer.rows() => self.cursor.y += 1,
            Action::Right | Action::Left => {
                self.cursor.x = self.cursor.x.min(self.buffer.cols(self.cursor.y) - 1)
            }
            _ => {}
        }

        {
            let mut offset = self.offset;
            offset = offset.min(self.cursor.y);
            if self.cursor.y + 1 >= self.scrsiz.height {
                offset = offset.max(self.cursor.y + 1 - self.scrsiz.height);
            }
            if self.offset != offset {
                self.offset = offset;
                all = true;
            }
        }

        let mut out = all
            .then(|| Vec::<u8>::with_capacity(self.scrsiz.width * self.scrsiz.height * 4))
            .unwrap_or_default();

        let mut cur: Option<Point> = None;
        'outer: loop {
            out.clear();
            let mut yet = true;

            let mut pos = Point::new(0, 0);
            for (lpt, lbr) in self.buffer.line.iter().enumerate().skip(self.offset) {
                pos.x = 0;
                let mut end = 0;

                let mut bgn_pre = 0;
                let mut pos_pre = pos;
                for (cpt, (str, seg)) in lbr.span().enumerate() {
                    if cur.is_none() && lpt == self.cursor.y && seg.contains(&self.cursor.x) {
                        match action {
                            Action::Right
                                if yet && self.cursor.x + 1 < self.buffer.cols(self.cursor.y) =>
                            {
                                self.cursor.x = seg.end;
                                yet = false; // cur will be determined by the next iteration
                            }
                            Action::Left if 0 < self.cursor.x => {
                                self.cursor.x = if self.cursor.x > seg.start {
                                    seg.start
                                } else {
                                    bgn_pre
                                };
                                cur = Some(pos_pre);
                            }
                            _ => cur = Some(pos),
                        }
                    }
                    if cur.is_some() && !all {
                        break 'outer;
                    }
                    if cpt == lbr.span.len() - 1 {
                        break;
                    }

                    // save for Action::Left
                    pos_pre = pos;
                    bgn_pre = seg.start;

                    pos.x += seg.len();
                    if pos.x >= self.scrsiz.width {
                        pos.x = 0;
                        pos.y += 1;
                        if pos.y >= self.scrsiz.height {
                            break;
                        }
                    }
                    end = str.end;
                }
                if cur.is_none()
                    && lpt == self.cursor.y
                    && (Action::Up == action || Action::Down == action)
                {
                    cur = Some(pos)
                }
                if all {
                    out.extend(&lbr.text.as_slice()[..end]);
                }
                pos.y += 1;
                if pos.y >= self.scrsiz.height {
                    break;
                }
                if all {
                    out.extend(b"\r\n");
                }
            }
            if cur.is_some() {
                break;
            }
            self.offset += 1;
            all = true;
        }
        let cur = unsafe { cur.unwrap_unchecked() };

        if all {
            execute!(
                stdout(),
                cursor::Hide,
                terminal::Clear(terminal::ClearType::All),
                cursor::MoveTo(0, 0),
                Print(unsafe { std::str::from_utf8_unchecked(&out) }),
                cursor::MoveTo(cur.x as _, cur.y as _),
                cursor::Show
            )?;
        } else {
            execute!(stdout(), cursor::MoveTo(cur.x as _, cur.y as _),)?;
        }

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
            Event::Resize(cols, rows) => Action::Resize(Size::new(cols as _, rows as _)),
            _ => continue,
        };
        editor.draw(action)?;
    }
    Screen::fini()?;
    Ok(())
}

#[cfg(test)]
include!("test.rs");
