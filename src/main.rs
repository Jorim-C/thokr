mod lang;
mod thok;
mod ui;
mod util;

use crate::{lang::Language, thok::Thok};
use clap::{ArgEnum, ErrorKind, IntoApp, Parser};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    tty::IsTty,
};
use std::{
    error::Error,
    io::{self, stdin},
    sync::mpsc,
    thread,
    time::Duration,
};
use tui::{
    backend::{Backend, CrosstermBackend},
    Frame, Terminal,
};
use webbrowser::Browser;

const TICK_RATE_MS: u64 = 100;
const TIMER_STOP_DELAY_SECS: u64 = 5;

/// sleek typing tui with visualized results and historical logging
#[derive(Parser, Debug, Clone)]
#[clap(version, about, long_about= None)]
pub struct Cli {
    /// number of words to use in test
    #[clap(short = 'w', long, default_value_t = 15)]
    number_of_words: usize,

    /// number of sentences to use in test
    #[clap(short = 'f', long = "full-sentences")]
    number_of_sentences: Option<usize>,

    /// number of seconds to run test
    #[clap(short = 's', long)]
    number_of_secs: Option<i64>,

    /// custom prompt to use
    #[clap(short = 'p', long)]
    prompt: Option<String>,

    /// language to pull words from
    #[clap(short = 'l', long, arg_enum, default_value_t = SupportedLanguage::English)]
    supported_language: SupportedLanguage,
}

#[derive(Debug, Copy, Clone, ArgEnum, strum_macros::Display)]
enum SupportedLanguage {
    English,
    English1k,
    English10k,
}

impl SupportedLanguage {
    fn as_lang(&self) -> Language {
        Language::new(self.to_string().to_lowercase())
    }
}

#[derive(Debug)]
struct App {
    cli: Option<Cli>,
    thok: Thok,
    timer_id: u64,
}

impl App {
    fn new(cli: Cli) -> Self {
        let mut count = 0;
        let prompt = if cli.prompt.is_some() {
            cli.prompt.clone().unwrap()
        } else if cli.number_of_sentences.is_some() {
            let language = cli.supported_language.as_lang();
            let (s, count_tmp) = language.get_random_sentence(cli.number_of_sentences.unwrap());
            count = count_tmp;
            // sets the word count for the sentence.
            s.join("")
        } else {
            let language = cli.supported_language.as_lang();

            language.get_random(cli.number_of_words).join(" ")
        };
        if cli.number_of_sentences.is_some() {
            Self {
                thok: Thok::new(prompt, count, cli.number_of_secs),
                cli: Some(cli),
                timer_id: 0,
            }
        } else {
            Self {
                thok: Thok::new(
                    prompt,
                    cli.number_of_words,
                    cli.number_of_secs,
                ),
                cli: Some(cli),
                timer_id: 0,
            }
        }
    }

    fn reset(&mut self, new_prompt: Option<String>) {
        self.timer_id = self.timer_id.wrapping_add(1);
        let cli = self.cli.clone().unwrap();
        let mut count = 0;
        let prompt = match new_prompt {
            Some(_) => new_prompt.unwrap(),
            _ => match cli.number_of_sentences {
                Some(t) => {
                    let language = cli.supported_language.as_lang();
                    let (s, count_tmp) = language.get_random_sentence(t);
                    count = count_tmp;
                    // sets the word count for the sentence
                    s.join("")
                }
                _ => {
                    let language = cli.supported_language.as_lang();
                    language.get_random(cli.number_of_words).join(" ")
                }
            },
        };
        if cli.number_of_sentences.is_some() {
            self.thok = Thok::new(prompt, count, cli.number_of_secs);
        } else {
            self.thok = Thok::new(
                prompt,
                cli.number_of_words,
                cli.number_of_secs,
            );
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    if !stdin().is_tty() {
        let mut cmd = Cli::command();
        cmd.error(ErrorKind::Io, "stdin must be a tty").exit();
    }

    enable_raw_mode()?;

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(cli);
    start_tui(&mut terminal, &mut app)?;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen,)?;
    terminal.show_cursor()?;

    Ok(())
}

enum ExitType {
    Restart,
    New,
    Quit,
}
fn start_tui<B: Backend>(
    terminal: &mut Terminal<B>,
    mut app: &mut App,
) -> Result<(), Box<dyn Error>> {
    let (thok_events, timer_commands) = get_thok_events();

    loop {
        let mut exit_type: ExitType = ExitType::Quit;
        terminal.draw(|f| ui(app, f))?;

        loop {
            let app = &mut app;

            match thok_events.recv()? {
                ThokEvent::Timer {
                    timer_id,
                    milliseconds_remaining,
                } => {
                    if timer_id == app.timer_id
                        && app.thok.has_started()
                        && !app.thok.has_finished()
                    {
                        app.thok.set_milliseconds_remaining(milliseconds_remaining);

                        if app.thok.has_finished() {
                            app.thok.calc_results();
                        }
                        terminal.draw(|f| ui(app, f))?;
                    }
                }
                ThokEvent::TimerExpired { timer_id } => {
                    if timer_id == app.timer_id
                        && app.thok.has_started()
                        && !app.thok.has_finished()
                    {
                        thread::sleep(Duration::from_secs(TIMER_STOP_DELAY_SECS));
                        app.thok.set_milliseconds_remaining(0);
                        app.thok.calc_results();
                        terminal.draw(|f| ui(app, f))?;
                    }
                }
                ThokEvent::Resize => {
                    terminal.draw(|f| ui(app, f))?;
                }
                ThokEvent::Key(key) => {
                    match key.code {
                        KeyCode::Esc => {
                            break;
                        }
                        KeyCode::Backspace => {
                            if !app.thok.has_finished() {
                                app.thok.backspace();
                            }
                        }
                        KeyCode::Left => {
                            exit_type = ExitType::Restart;
                            break;
                        }
                        KeyCode::Right => {
                            exit_type = ExitType::New;
                            break;
                        }
                        KeyCode::Char(c) => {
                            if key.modifiers.contains(KeyModifiers::CONTROL)
                                && key.code == KeyCode::Char('c')
                            // ctrl+c to quit
                            {
                                break;
                            }

                            match app.thok.has_finished() {
                                false => {
                                    let had_started = app.thok.has_started();
                                    app.thok.write(c);
                                    if !had_started {
                                        if let Some(milliseconds) = app.thok.milliseconds_remaining {
                                            let _ = timer_commands.send(TimerCommand::Start {
                                                timer_id: app.timer_id,
                                                milliseconds,
                                            });
                                            drop(thok_events);
                                            thread::sleep(Duration::from_millis(TICK_RATE_MS * 2));
                                            return Ok(());
                                        }
                                    }
                                    if app.thok.has_finished() {
                                        app.thok.calc_results();
                                        let _ = timer_commands.send(TimerCommand::Stop);
                                    }
                                }
                                true => match key.code {
                                    KeyCode::Char('t') => {
                                        if Browser::is_available() {
                                            webbrowser::open(&format!("https://twitter.com/intent/tweet?text={}%20wpm%20%2F%20{}%25%20acc%20%2F%20{:.2}%20sd%0A%0Ahttps%3A%2F%2Fgithub.com%thatvegandev%2Fthokr", app.thok.wpm, app.thok.accuracy, app.thok.std_dev))
                                    .unwrap_or_default();
                                        }
                                    }
                                    KeyCode::Char('r') => {
                                        exit_type = ExitType::Restart;
                                        break;
                                    }
                                    KeyCode::Char('n') => {
                                        exit_type = ExitType::New;
                                        break;
                                    }
                                    _ => {}
                                },
                            }
                        }
                        _ => {}
                    }
                    terminal.draw(|f| ui(app, f))?;
                }
            }
        }

        if app.thok.has_started() && !app.thok.has_finished() {
            thread::sleep(Duration::from_secs(TIMER_STOP_DELAY_SECS));
        }
        let _ = timer_commands.send(TimerCommand::Stop);
        match exit_type {
            ExitType::Restart => {
                app.reset(Some(app.thok.prompt.clone()));
            }
            ExitType::New => {
                app.reset(None);
            }
            ExitType::Quit => {
                break;
            }
        }
    }

    Ok(())
}

#[derive(Clone)]
enum ThokEvent {
    Key(KeyEvent),
    Resize,
    Timer {
        timer_id: u64,
        milliseconds_remaining: i64,
    },
    TimerExpired {
        timer_id: u64,
    },
}

enum TimerCommand {
    Start { timer_id: u64, milliseconds: i64 },
    Stop,
}

fn get_thok_events() -> (mpsc::Receiver<ThokEvent>, mpsc::Sender<TimerCommand>) {
    let (event_tx, event_rx) = mpsc::channel();
    let (timer_tx, timer_rx) = mpsc::channel();

    let timer_events = event_tx.clone();
    thread::spawn(move || {
        let mut active_timer = None;

        loop {
            match timer_rx.recv_timeout(Duration::from_millis(TICK_RATE_MS)) {
                Ok(TimerCommand::Start {
                    timer_id,
                    milliseconds,
                }) => active_timer = Some((timer_id, milliseconds)),
                Ok(TimerCommand::Stop) => active_timer = None,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some((timer_id, milliseconds)) = active_timer {
                        let remaining = milliseconds.saturating_sub(TICK_RATE_MS as i64);
                        if remaining == 0 {
                            timer_events
                                .send(ThokEvent::TimerExpired { timer_id })
                                .unwrap();
                            thread::sleep(Duration::from_secs(TIMER_STOP_DELAY_SECS));
                            active_timer = None;
                        } else {
                            timer_events
                                .send(ThokEvent::Timer {
                                    timer_id,
                                    milliseconds_remaining: remaining,
                                })
                                .unwrap();
                            active_timer = Some((timer_id, remaining));
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    thread::spawn(move || loop {
        let evt = match event::read().unwrap() {
            Event::Key(key) => Some(ThokEvent::Key(key)),
            Event::Resize(_, _) => Some(ThokEvent::Resize),
            _ => None,
        };

        if evt.is_some() && event_tx.send(evt.unwrap()).is_err() {
            break;
        }
    });

    (event_rx, timer_tx)
}

fn ui<B: Backend>(app: &mut App, f: &mut Frame<B>) {
    f.render_widget(&app.thok, f.size());
}
