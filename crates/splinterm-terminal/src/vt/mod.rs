//! Streaming VT recognizer derived from Foot 1.27.0 `vt.c`, `vt.h`, and
//! parser storage in `terminal.h` at commit
//! `3c5b584b0eafa772eb4376fb6eaf6643399e190e`.
//!
//! Recognition remains byte-streaming and chunk-independent. CSI storage is
//! fixed at Foot's 16 parameters and 16 subparameters. OSC and DCS collection
//! is intentionally bounded; overflow consumes through the terminator without
//! desynchronizing subsequent input.

const PARAM_LIMIT: usize = 16;
const SUBPARAM_LIMIT: usize = 16;
const INTERMEDIATE_LIMIT: usize = 4;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Param {
    value: u32,
    present: bool,
    subparams: [u32; SUBPARAM_LIMIT],
    subparam_present: [bool; SUBPARAM_LIMIT],
    subparam_count: usize,
}

impl Param {
    pub(crate) fn value(self, default: u32, zero_is_default: bool) -> u32 {
        if !self.present || (zero_is_default && self.value == 0) {
            default
        } else {
            self.value & 0x7fff_ffff
        }
    }

    pub(crate) fn subparam(self, index: usize) -> Option<u32> {
        (index < self.subparam_count && self.subparam_present[index])
            .then_some(self.subparams[index] & 0x7fff_ffff)
    }

    pub(crate) const fn subparam_count(self) -> usize {
        self.subparam_count
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Params {
    values: [Param; PARAM_LIMIT],
    count: usize,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            values: [Param::default(); PARAM_LIMIT],
            count: 0,
        }
    }
}

impl Params {
    pub(crate) fn get(&self, index: usize) -> Param {
        if index < self.count {
            self.values[index]
        } else {
            Param::default()
        }
    }

    pub(crate) const fn count(&self) -> usize {
        self.count
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StringTerminator {
    Bell,
    StringTerminator,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Action {
    Print(char),
    Execute(u8),
    Esc {
        intermediates: [u8; INTERMEDIATE_LIMIT],
        intermediate_count: usize,
        final_byte: u8,
    },
    Csi {
        private: Option<u8>,
        intermediates: [u8; INTERMEDIATE_LIMIT],
        intermediate_count: usize,
        params: Box<Params>,
        final_byte: u8,
    },
    Osc(Vec<u8>, StringTerminator),
    Dcs(Vec<u8>),
    SixelBegin(Box<Params>),
    SixelData(u8),
    SixelEnd,
    SixelAbort,
    KittyBegin(Vec<u8>, bool),
    KittyCommand(Vec<u8>, bool),
    KittyData(u8),
    KittyEnd,
    KittyAbort,
    StringTruncated(&'static str),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum State {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    OscString,
    DcsEntry,
    DcsParam,
    DcsIntermediate,
    DcsIgnore,
    DcsPassthrough,
    DcsEscape,
    SosPmApcString,
    SosPmApcEscape,
    ApcPrefix,
    KittyControl,
    KittyControlEscape,
    KittyPayload,
    KittyPayloadEscape,
    Utf8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Parser {
    state: State,
    private: Option<u8>,
    intermediates: [u8; INTERMEDIATE_LIMIT],
    intermediate_count: usize,
    params: Params,
    current_param: usize,
    current_subparam: Option<usize>,
    parameter_overflow: bool,
    string: Vec<u8>,
    string_limit: usize,
    string_truncated: bool,
    osc_limit: usize,
    dcs_limit: usize,
    sixel_streaming: bool,
    utf8_value: u32,
    utf8_minimum: u32,
    utf8_remaining: u8,
}

impl Parser {
    pub(crate) fn new(osc_limit: usize, dcs_limit: usize) -> Self {
        Self {
            state: State::Ground,
            private: None,
            intermediates: [0; INTERMEDIATE_LIMIT],
            intermediate_count: 0,
            params: Params::default(),
            current_param: 0,
            current_subparam: None,
            parameter_overflow: false,
            string: Vec::new(),
            string_limit: 0,
            string_truncated: false,
            osc_limit,
            dcs_limit,
            sixel_streaming: false,
            utf8_value: 0,
            utf8_minimum: 0,
            utf8_remaining: 0,
        }
    }

    /// Returns an action and whether the same byte must be reprocessed.
    pub(crate) fn feed(&mut self, byte: u8) -> (Option<Action>, bool) {
        if self.state == State::Utf8 {
            return self.feed_utf8(byte);
        }

        if matches!(byte, 0x18 | 0x1a) {
            let action = if self.sixel_streaming {
                Action::SixelAbort
            } else {
                Action::KittyAbort
            };
            self.state = State::Ground;
            self.sixel_streaming = false;
            self.string.clear();
            self.clear_sequence();
            return (Some(action), false);
        }
        if byte == 0x1b
            && !matches!(
                self.state,
                State::OscString
                    | State::DcsPassthrough
                    | State::DcsEscape
                    | State::SosPmApcString
                    | State::SosPmApcEscape
                    | State::ApcPrefix
                    | State::KittyControl
                    | State::KittyControlEscape
                    | State::KittyPayload
                    | State::KittyPayloadEscape
            )
        {
            self.clear_sequence();
            self.state = State::Escape;
            return (None, false);
        }

        match self.state {
            State::Ground => self.feed_ground(byte),
            State::Escape | State::EscapeIntermediate => self.feed_escape(byte),
            State::CsiEntry | State::CsiParam | State::CsiIntermediate | State::CsiIgnore => {
                self.feed_csi(byte)
            }
            State::OscString => self.feed_osc(byte),
            State::DcsEntry
            | State::DcsParam
            | State::DcsIntermediate
            | State::DcsIgnore
            | State::DcsPassthrough
            | State::DcsEscape => self.feed_dcs(byte),
            State::SosPmApcString | State::SosPmApcEscape => self.feed_ignored_string(byte),
            State::ApcPrefix
            | State::KittyControl
            | State::KittyControlEscape
            | State::KittyPayload
            | State::KittyPayloadEscape => self.feed_apc(byte),
            State::Utf8 => unreachable!(),
        }
    }

    fn feed_ground(&mut self, byte: u8) -> (Option<Action>, bool) {
        match byte {
            0x00..=0x1f => (Some(Action::Execute(byte)), false),
            0x20..=0x7e => (Some(Action::Print(char::from(byte))), false),
            0xc2..=0xdf => {
                self.start_utf8(u32::from(byte & 0x1f), 1, 0x80);
                (None, false)
            }
            0xe0..=0xef => {
                self.start_utf8(u32::from(byte & 0x0f), 2, 0x800);
                (None, false)
            }
            0xf0..=0xf4 => {
                self.start_utf8(u32::from(byte & 0x07), 3, 0x1_0000);
                (None, false)
            }
            0x9f => {
                self.state = State::ApcPrefix;
                (None, false)
            }
            _ => (None, false),
        }
    }

    fn feed_escape(&mut self, byte: u8) -> (Option<Action>, bool) {
        if byte == 0x7f {
            return (None, false);
        }
        match byte {
            0x00..=0x1f => (Some(Action::Execute(byte)), false),
            b'[' if self.state == State::Escape => {
                self.clear_sequence();
                self.state = State::CsiEntry;
                (None, false)
            }
            b']' if self.state == State::Escape => {
                self.start_string(State::OscString, self.osc_limit);
                (None, false)
            }
            b'P' if self.state == State::Escape => {
                self.clear_sequence();
                self.state = State::DcsEntry;
                (None, false)
            }
            b'X' | b'^' if self.state == State::Escape => {
                self.state = State::SosPmApcString;
                (None, false)
            }
            b'_' if self.state == State::Escape => {
                self.state = State::ApcPrefix;
                (None, false)
            }
            0x20..=0x2f => {
                self.collect_intermediate(byte);
                self.state = State::EscapeIntermediate;
                (None, false)
            }
            0x30..=0x7e => {
                let action = Action::Esc {
                    intermediates: self.intermediates,
                    intermediate_count: self.intermediate_count,
                    final_byte: byte,
                };
                self.state = State::Ground;
                (Some(action), false)
            }
            _ => {
                self.state = State::Ground;
                (None, false)
            }
        }
    }

    fn feed_csi(&mut self, byte: u8) -> (Option<Action>, bool) {
        if byte == 0x7f {
            return (None, false);
        }
        if byte <= 0x1f {
            return (Some(Action::Execute(byte)), false);
        }
        if self.state == State::CsiIgnore {
            if (0x40..=0x7e).contains(&byte) {
                self.state = State::Ground;
            }
            return (None, false);
        }
        if self.state == State::CsiIntermediate && (0x30..=0x3f).contains(&byte) {
            self.state = State::CsiIgnore;
            return (None, false);
        }
        match byte {
            0x30..=0x39 => {
                self.collect_digit(byte - b'0');
                self.state = State::CsiParam;
                (None, false)
            }
            b';' => {
                self.next_param();
                self.state = if self.parameter_overflow {
                    State::CsiIgnore
                } else {
                    State::CsiParam
                };
                (None, false)
            }
            b':' => {
                self.next_subparam();
                self.state = State::CsiParam;
                (None, false)
            }
            0x3c..=0x3f if self.state == State::CsiEntry && self.private.is_none() => {
                self.private = Some(byte);
                (None, false)
            }
            0x20..=0x2f => {
                self.collect_intermediate(byte);
                self.state = State::CsiIntermediate;
                (None, false)
            }
            0x40..=0x7e => {
                self.finish_params();
                let action = Action::Csi {
                    private: self.private,
                    intermediates: self.intermediates,
                    intermediate_count: self.intermediate_count,
                    params: Box::new(self.params),
                    final_byte: byte,
                };
                self.state = State::Ground;
                (Some(action), false)
            }
            _ => {
                self.state = State::CsiIgnore;
                (None, false)
            }
        }
    }

    fn feed_osc(&mut self, byte: u8) -> (Option<Action>, bool) {
        match byte {
            0x07 => self.finish_string(true, StringTerminator::Bell),
            0x1b => {
                let result = self.finish_string(true, StringTerminator::StringTerminator);
                self.clear_sequence();
                self.state = State::Escape;
                result
            }
            0x00..=0x1f | 0x7f => (None, false),
            _ => {
                self.push_string(byte);
                (None, false)
            }
        }
    }

    fn feed_dcs(&mut self, byte: u8) -> (Option<Action>, bool) {
        match self.state {
            State::DcsPassthrough => match byte {
                0x1b => {
                    self.state = State::DcsEscape;
                    (None, false)
                }
                0x7f => (None, false),
                0x80..=0x9f => self.finish_dcs(),
                _ if self.sixel_streaming => (Some(Action::SixelData(byte)), false),
                _ => {
                    self.push_string(byte);
                    (None, false)
                }
            },
            State::DcsEscape => {
                if byte == b'\\' {
                    self.finish_dcs()
                } else {
                    let action = self.sixel_streaming.then_some(Action::SixelAbort);
                    self.sixel_streaming = false;
                    self.state = State::Escape;
                    self.clear_sequence();
                    (action, true)
                }
            }
            State::DcsIgnore => {
                if byte == 0x1b {
                    self.state = State::DcsEscape;
                }
                (None, false)
            }
            _ => {
                if byte <= 0x1f {
                    return (None, false);
                }
                match byte {
                    0x30..=0x39 => {
                        self.collect_digit(byte - b'0');
                        self.state = State::DcsParam;
                    }
                    b';' => {
                        self.next_param();
                        self.state = if self.parameter_overflow {
                            State::DcsIgnore
                        } else {
                            State::DcsParam
                        };
                    }
                    b':' => {
                        self.next_subparam();
                        self.state = State::DcsParam;
                    }
                    0x3c..=0x3f if self.state == State::DcsEntry => self.private = Some(byte),
                    0x20..=0x2f => {
                        self.collect_intermediate(byte);
                        self.state = State::DcsIntermediate;
                    }
                    0x40..=0x7e => {
                        self.finish_params();
                        if byte == b'q' && self.private.is_none() && self.intermediate_count == 0 {
                            self.state = State::DcsPassthrough;
                            self.sixel_streaming = true;
                            return (Some(Action::SixelBegin(Box::new(self.params))), false);
                        }
                        self.start_string(State::DcsPassthrough, self.dcs_limit);
                    }
                    _ => self.state = State::DcsIgnore,
                }
                (None, false)
            }
        }
    }

    fn feed_apc(&mut self, byte: u8) -> (Option<Action>, bool) {
        match self.state {
            State::ApcPrefix => {
                if byte == b'G' {
                    self.start_string(State::KittyControl, 1024);
                } else if byte == 0x1b {
                    self.state = State::SosPmApcEscape;
                } else if byte == 0x9c {
                    self.state = State::Ground;
                } else {
                    self.state = State::SosPmApcString;
                }
                (None, false)
            }
            State::KittyControl => match byte {
                b';' => {
                    let control = std::mem::take(&mut self.string);
                    let truncated = std::mem::take(&mut self.string_truncated);
                    self.state = State::KittyPayload;
                    (Some(Action::KittyBegin(control, truncated)), false)
                }
                0x1b => {
                    self.state = State::KittyControlEscape;
                    (None, false)
                }
                0x9c => self.finish_kitty_command(),
                _ => {
                    self.push_string(byte);
                    (None, false)
                }
            },
            State::KittyPayload => match byte {
                0x1b => {
                    self.state = State::KittyPayloadEscape;
                    (None, false)
                }
                0x9c => {
                    self.state = State::Ground;
                    (Some(Action::KittyEnd), false)
                }
                _ => (Some(Action::KittyData(byte)), false),
            },
            State::KittyControlEscape => {
                if byte == b'\\' {
                    self.finish_kitty_command()
                } else {
                    self.state = State::Escape;
                    self.string.clear();
                    (Some(Action::KittyAbort), true)
                }
            }
            State::KittyPayloadEscape => {
                if byte == b'\\' {
                    self.state = State::Ground;
                    (Some(Action::KittyEnd), false)
                } else {
                    self.state = State::Escape;
                    (Some(Action::KittyAbort), true)
                }
            }
            _ => unreachable!(),
        }
    }

    fn finish_kitty_command(&mut self) -> (Option<Action>, bool) {
        self.state = State::Ground;
        let control = std::mem::take(&mut self.string);
        let truncated = std::mem::take(&mut self.string_truncated);
        (Some(Action::KittyCommand(control, truncated)), false)
    }

    fn feed_ignored_string(&mut self, byte: u8) -> (Option<Action>, bool) {
        if byte == 0x9c {
            self.state = State::Ground;
        } else if self.state == State::SosPmApcEscape {
            if byte == b'\\' {
                self.state = State::Ground;
                return (None, false);
            }
            self.state = State::SosPmApcString;
        } else if byte == 0x1b {
            self.state = State::SosPmApcEscape;
        }
        (None, false)
    }

    fn feed_utf8(&mut self, byte: u8) -> (Option<Action>, bool) {
        if byte & 0xc0 != 0x80 {
            self.state = State::Ground;
            return (Some(Action::Print('\u{fffd}')), true);
        }
        self.utf8_value = (self.utf8_value << 6) | u32::from(byte & 0x3f);
        self.utf8_remaining -= 1;
        if self.utf8_remaining > 0 {
            return (None, false);
        }
        self.state = State::Ground;
        let value = self.utf8_value;
        if value < self.utf8_minimum || value > 0x10_ffff || (0xd800..=0xdfff).contains(&value) {
            return (Some(Action::Print('\u{fffd}')), false);
        }
        (char::from_u32(value).map(Action::Print), false)
    }

    fn start_utf8(&mut self, value: u32, remaining: u8, minimum: u32) {
        self.state = State::Utf8;
        self.utf8_value = value;
        self.utf8_remaining = remaining;
        self.utf8_minimum = minimum;
    }

    fn start_string(&mut self, state: State, limit: usize) {
        self.state = state;
        self.string.clear();
        self.string_limit = limit;
        self.string_truncated = false;
    }

    fn push_string(&mut self, byte: u8) {
        if self.string.len() < self.string_limit {
            self.string.push(byte);
        } else {
            self.string_truncated = true;
        }
    }

    fn finish_string(&mut self, osc: bool, terminator: StringTerminator) -> (Option<Action>, bool) {
        self.state = State::Ground;
        if self.string_truncated {
            self.string.clear();
            return (
                Some(Action::StringTruncated(if osc { "OSC" } else { "DCS" })),
                false,
            );
        }
        let payload = std::mem::take(&mut self.string);
        (Some(Action::Osc(payload, terminator)), false)
    }

    fn finish_dcs(&mut self) -> (Option<Action>, bool) {
        self.state = State::Ground;
        if self.sixel_streaming {
            self.sixel_streaming = false;
            self.clear_sequence();
            return (Some(Action::SixelEnd), false);
        }
        if self.string_truncated {
            self.string.clear();
            (Some(Action::StringTruncated("DCS")), false)
        } else {
            (Some(Action::Dcs(std::mem::take(&mut self.string))), false)
        }
    }

    fn clear_sequence(&mut self) {
        self.private = None;
        self.intermediates = [0; INTERMEDIATE_LIMIT];
        self.intermediate_count = 0;
        self.params = Params::default();
        self.current_param = 0;
        self.current_subparam = None;
        self.parameter_overflow = false;
    }

    fn collect_intermediate(&mut self, byte: u8) {
        if self.intermediate_count < INTERMEDIATE_LIMIT {
            self.intermediates[self.intermediate_count] = byte;
            self.intermediate_count += 1;
        }
    }

    fn collect_digit(&mut self, digit: u8) {
        if self.current_param >= PARAM_LIMIT {
            return;
        }
        self.params.count = self.params.count.max(self.current_param + 1);
        let param = &mut self.params.values[self.current_param];
        if let Some(subparam) = self.current_subparam {
            if subparam < SUBPARAM_LIMIT {
                param.subparam_count = param.subparam_count.max(subparam + 1);
                param.subparam_present[subparam] = true;
                param.subparams[subparam] = param.subparams[subparam]
                    .wrapping_mul(10)
                    .wrapping_add(u32::from(digit));
            }
        } else {
            param.present = true;
            param.value = param.value.wrapping_mul(10).wrapping_add(u32::from(digit));
        }
    }

    fn next_param(&mut self) {
        self.params.count = self
            .params
            .count
            .max((self.current_param + 1).min(PARAM_LIMIT));
        if self.current_param + 1 >= PARAM_LIMIT {
            self.parameter_overflow = true;
        } else {
            self.current_param += 1;
        }
        self.current_subparam = None;
    }

    fn next_subparam(&mut self) {
        if self.current_param >= PARAM_LIMIT {
            return;
        }
        self.params.count = self.params.count.max(self.current_param + 1);
        let param = &mut self.params.values[self.current_param];
        let next = self
            .current_subparam
            .map_or(0, |index| index.saturating_add(1));
        self.current_subparam = Some(next);
        param.subparam_count = param.subparam_count.max((next + 1).min(SUBPARAM_LIMIT));
    }

    fn finish_params(&mut self) {
        if self.current_param < PARAM_LIMIT
            && (self.params.count > 0 || self.current_subparam.is_some())
        {
            self.params.count = self.params.count.max(self.current_param + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_utf8_replacement_reprocesses_ascii() {
        let mut parser = Parser::new(16, 16);
        assert_eq!(parser.feed(0xe2), (None, false));
        assert_eq!(parser.feed(b'A'), (Some(Action::Print('\u{fffd}')), true));
        assert_eq!(parser.feed(b'A'), (Some(Action::Print('A')), false));
    }

    #[test]
    fn osc_overflow_stays_synchronized() {
        let mut parser = Parser::new(2, 2);
        for byte in b"\x1b]2;abc\x07" {
            let _ = parser.feed(*byte);
        }
        assert_eq!(parser.state, State::Ground);
        assert_eq!(parser.feed(b'Z'), (Some(Action::Print('Z')), false));
    }

    #[test]
    fn sixel_is_streamed_without_using_the_collected_dcs_limit() {
        let mut parser = Parser::new(2, 1);
        let mut actions = Vec::new();
        for byte in b"\x1bP7;1;0q#1~\x1b\\" {
            let (action, again) = parser.feed(*byte);
            assert!(!again);
            if let Some(action) = action {
                actions.push(action);
            }
        }
        assert_eq!(actions.len(), 5);
        let Action::SixelBegin(params) = &actions[0] else {
            panic!("expected streaming Sixel begin");
        };
        assert_eq!(params.get(0).value(0, false), 7);
        assert_eq!(params.get(1).value(0, false), 1);
        assert_eq!(params.get(2).value(1, false), 0);
        assert_eq!(actions[1], Action::SixelData(b'#'));
        assert_eq!(actions[2], Action::SixelData(b'1'));
        assert_eq!(actions[3], Action::SixelData(b'~'));
        assert_eq!(actions[4], Action::SixelEnd);
    }

    #[test]
    fn kitty_apc_streams_only_recognized_payloads_and_recovers() {
        let mut parser = Parser::new(16, 16);
        let mut actions = Vec::new();
        for byte in b"\x1b_not-kitty\x1b\\Z\x1b_Gi=7,m=0;AAAA\x1b\\Q" {
            let (action, mut again) = parser.feed(*byte);
            if let Some(action) = action {
                actions.push(action);
            }
            while again {
                let (action, next) = parser.feed(*byte);
                if let Some(action) = action {
                    actions.push(action);
                }
                again = next;
            }
        }
        assert_eq!(
            actions,
            vec![
                Action::Print('Z'),
                Action::KittyBegin(b"i=7,m=0".to_vec(), false),
                Action::KittyData(b'A'),
                Action::KittyData(b'A'),
                Action::KittyData(b'A'),
                Action::KittyData(b'A'),
                Action::KittyEnd,
                Action::Print('Q'),
            ]
        );
    }

    #[test]
    fn kitty_control_only_and_cancel_are_bounded_and_synchronized() {
        let mut parser = Parser::new(16, 16);
        let mut actions = Vec::new();
        let mut input = b"\x1b_Ga=p,i=7\x1b\\\x1b_G".to_vec();
        input.extend(std::iter::repeat_n(b'x', 1025));
        input.extend_from_slice(b"\x1b\\\x1b_Ga=t;AAAA\x1aZ");
        for byte in input {
            let (action, mut again) = parser.feed(byte);
            if let Some(action) = action {
                actions.push(action);
            }
            while again {
                let (action, next) = parser.feed(byte);
                if let Some(action) = action {
                    actions.push(action);
                }
                again = next;
            }
        }
        assert_eq!(
            actions.first(),
            Some(&Action::KittyCommand(b"a=p,i=7".to_vec(), false))
        );
        assert!(
            matches!(actions.get(1), Some(Action::KittyCommand(control, true)) if control.len() == 1024)
        );
        assert_eq!(actions.last(), Some(&Action::Print('Z')));
        assert!(actions.contains(&Action::KittyAbort));
        assert_eq!(parser.state, State::Ground);
    }

    #[test]
    fn sixel_cancel_aborts_and_recovers_to_ground() {
        let mut parser = Parser::new(16, 16);
        let mut actions = Vec::new();
        for byte in b"\x1bPq~\x18Z" {
            let (action, mut again) = parser.feed(*byte);
            if let Some(action) = action {
                actions.push(action);
            }
            while again {
                let (action, next) = parser.feed(*byte);
                if let Some(action) = action {
                    actions.push(action);
                }
                again = next;
            }
        }
        assert_eq!(
            actions,
            vec![
                Action::SixelBegin(Box::default()),
                Action::SixelData(b'~'),
                Action::SixelAbort,
                Action::Print('Z'),
            ]
        );
        assert_eq!(parser.state, State::Ground);
    }
}
