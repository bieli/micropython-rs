//! The `buttons` module: debounced button events with short and long presses.
//!
//! ```python
//! import buttons
//! while True:
//!     event = buttons.wait()
//!     if event == buttons.LEFT:        # short press, sent on release
//!         ...
//!     elif event == buttons.RIGHT_LONG:  # held for LONG_MS, sent while held
//!         ...
//! ```
//!
//! Buttons are active low (pressed = 0). A level must be stable for
//! `DEBOUNCE_MS` before it counts. Events wait in a small queue, so presses
//! between two `get()` calls aren't lost as long as the script polls often.

use micropython_rs::{Arg, Error, HwFn, Module};

use crate::board::{now_ms, Board, IN, IN_PULLUP};

pub const NONE: i64 = 0;
pub const LEFT: i64 = 1;
pub const RIGHT: i64 = 2;
pub const LEFT_LONG: i64 = 3;
pub const RIGHT_LONG: i64 = 4;

const DEBOUNCE_MS: u64 = 20;
const LONG_MS: u64 = 600;
/// How often `wait()` samples the buttons.
const POLL_MS: i64 = 5;
const QUEUE_SIZE: usize = 8;

pub const BUTTONS_MODULE: Module = Module {
    name: "buttons",
    doc: "Debounced button events (short and long presses)",
    functions: &[
        HwFn::with_args(
            "wait",
            0,
            1,
            "[timeout_ms]",
            "Wait for an event; NONE on timeout",
        ),
        HwFn::new("get", "", "The next event, or NONE (doesn't wait)"),
        HwFn::new("pressed", "button", "1 while LEFT or RIGHT is held down"),
    ],
    constants: &[
        ("NONE", NONE),
        ("LEFT", LEFT),
        ("RIGHT", RIGHT),
        ("LEFT_LONG", LEFT_LONG),
        ("RIGHT_LONG", RIGHT_LONG),
        ("LONG_MS", LONG_MS as i64),
    ],
};

#[derive(Clone, Copy, Default)]
struct ButtonState {
    /// Debounced state: true while pressed.
    down: bool,
    /// Last raw reading, and when it last changed.
    raw: bool,
    raw_since: u64,
    /// When the current press started, and whether its long event was sent.
    pressed_at: u64,
    long_sent: bool,
}

/// One or two buttons: index 0 is LEFT, index 1 is RIGHT.
pub struct Buttons {
    pins: [Option<u8>; 2],
    state: [ButtonState; 2],
    queue: [i64; QUEUE_SIZE],
    queued: usize,
}

impl Buttons {
    /// `left` / `right` are GPIO numbers; `pull_up` enables the internal
    /// pull-up (GPIO34-39 have none, so their buttons need an external one).
    pub fn new(board: &mut Board<'_>, left: Option<(u8, bool)>, right: Option<(u8, bool)>) -> Self {
        let mut pins = [None; 2];
        for (slot, button) in pins.iter_mut().zip([left, right]) {
            if let Some((pin, pull_up)) = button {
                let mode = if pull_up { IN_PULLUP } else { IN };
                if board.pin_mode(pin, mode).is_ok() {
                    *slot = Some(pin);
                }
            }
        }
        Self {
            pins,
            state: [ButtonState::default(); 2],
            queue: [NONE; QUEUE_SIZE],
            queued: 0,
        }
    }

    /// Runs `buttons.<name>(args)`.
    pub fn call(
        &mut self,
        board: &mut Board<'_>,
        name: &str,
        args: &[Arg<'_>],
    ) -> Result<Option<i64>, Error> {
        match name {
            "get" => {
                self.poll(board)?;
                Ok(Some(self.pop()))
            }
            "wait" => {
                let timeout = match args.first() {
                    Some(arg) => Some(u64::try_from(arg.int()?).map_err(|_| Error::ValueError)?),
                    None => None,
                };
                let start = now_ms();
                loop {
                    self.poll(board)?;
                    let event = self.pop();
                    if event != NONE {
                        return Ok(Some(event));
                    }
                    if timeout.is_some_and(|t| now_ms() - start >= t) {
                        return Ok(Some(NONE));
                    }
                    // Feeds the watchdog and stops on Ctrl-C
                    board.sleep_ms(POLL_MS)?;
                }
            }
            "pressed" => {
                let index = match args[0].int()? {
                    LEFT => 0,
                    RIGHT => 1,
                    _ => return Err(Error::ValueError),
                };
                self.poll(board)?;
                Ok(Some(self.state[index].down as i64))
            }
            _ => Err(Error::UnknownIdentifier),
        }
    }

    /// Samples both buttons and queues the events they produce.
    fn poll(&mut self, board: &mut Board<'_>) -> Result<(), Error> {
        let now = now_ms();
        for index in 0..2 {
            let Some(pin) = self.pins[index] else {
                continue;
            };
            let pressed = !board.read_pin(pin)?; // active low
            let state = &mut self.state[index];
            if pressed != state.raw {
                state.raw = pressed;
                state.raw_since = now;
            }

            let mut event = NONE;
            if state.raw != state.down && now - state.raw_since >= DEBOUNCE_MS {
                state.down = state.raw;
                if state.down {
                    state.pressed_at = now;
                    state.long_sent = false;
                } else if !state.long_sent {
                    event = [LEFT, RIGHT][index];
                }
            }
            if state.down && !state.long_sent && now - state.pressed_at >= LONG_MS {
                state.long_sent = true;
                event = [LEFT_LONG, RIGHT_LONG][index];
            }
            if event != NONE {
                self.push(event);
            }
        }
        Ok(())
    }

    fn push(&mut self, event: i64) {
        if self.queued < QUEUE_SIZE {
            self.queue[self.queued] = event;
            self.queued += 1;
        }
    }

    fn pop(&mut self) -> i64 {
        if self.queued == 0 {
            return NONE;
        }
        let event = self.queue[0];
        self.queue.copy_within(1.., 0);
        self.queued -= 1;
        event
    }
}
