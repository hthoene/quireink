//! The microSD card, in SPI mode: blocks of 512 bytes read and written a
//! whole command at a time, with the card selected and the bus held for
//! each. The bus is shared with the display, so it comes in a `RefCell`
//! and is borrowed only for a command.
//!
//! A general library made every byte its own transaction (the card
//! selected and let go, the bus's clock started and stopped), and a block
//! read polls the card byte by byte until its data comes: on the X4 that
//! overhead took more of the time reading books than the bytes did.

use core::cell::RefCell;

use embedded_hal::delay::DelayNs;
use embedded_hal::digital::OutputPin;
use embedded_hal::spi::SpiBus;

/// Bytes in a block.
pub const BLOCK: usize = 512;

/// The card's answer to a command: idle (still starting).
const IDLE: u8 = 0x01;
/// The card's answer to a command: it does not know it (an old card).
const ILLEGAL: u8 = 0x04;
/// What comes before a block of data.
const DATA_TOKEN: u8 = 0xFE;
/// The card's answer to a written block: taken.
const ACCEPTED: u8 = 0x05;

/// Polls answered at once before waiting between them.
const QUICK_TRIES: u32 = 64;
/// Between polls after that, in microseconds.
const STEP_US: u32 = 10;
/// How long a card may take to send a block (cards promise 100 ms).
const READ_US: u32 = 200_000;
/// How long a card may take to write one (cards promise 250 ms) or to be
/// ready for a command.
const BUSY_US: u32 = 500_000;
/// How long a card may take to start.
const START_US: u32 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Spi,
    Pin,
    /// No card answered.
    NoCard,
    /// The card did not answer in time.
    Timeout,
    /// The card answered a command (the first) with an error (the second).
    Command(u8, u8),
    /// The card sent an error instead of data, or did not take a block.
    Data,
    /// A card this driver does not know (e.g. one for other voltages).
    Unsupported,
}

/// The card, on its share of the bus and its select pin.
#[derive(Debug)]
pub struct Card<'a, BUS, CS, D> {
    bus: &'a RefCell<BUS>,
    cs: CS,
    delay: D,
    /// Addressed by blocks (SDHC, SDXC), not bytes.
    high_capacity: bool,
    blocks: u32,
}

/// One command's time on the bus.
struct Session<'s, BUS, D> {
    bus: &'s mut BUS,
    delay: &'s mut D,
    tries: u32,
    waited: u32,
}

impl<BUS: SpiBus, D: DelayNs> Session<'_, BUS, D> {
    fn transfer(&mut self, bytes: &mut [u8]) -> Result<(), Error> {
        bytes.fill(0xFF);
        self.bus.transfer_in_place(bytes).map_err(|_| Error::Spi)
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.bus.write(bytes).map_err(|_| Error::Spi)
    }

    fn byte(&mut self) -> Result<u8, Error> {
        let mut byte = [0xFF];
        self.transfer(&mut byte)?;
        Ok(byte[0])
    }

    /// Starts counting the polls of a wait.
    fn start_wait(&mut self) {
        self.tries = 0;
        self.waited = 0;
    }

    /// After a poll that was not yet answered: waits a little, once the
    /// quick polls are used, until `limit_us`.
    fn wait(&mut self, limit_us: u32) -> Result<(), Error> {
        self.tries += 1;
        if self.tries > QUICK_TRIES {
            if self.waited >= limit_us {
                return Err(Error::Timeout);
            }
            self.delay.delay_us(STEP_US);
            self.waited += STEP_US;
        }
        Ok(())
    }

    /// Until the card no longer holds its data line low (busy).
    fn ready(&mut self, limit_us: u32) -> Result<(), Error> {
        self.start_wait();
        while self.byte()? != 0xFF {
            self.wait(limit_us)?;
        }
        Ok(())
    }

    /// Sends a command; the card's answer (its first byte).
    fn command(&mut self, command: u8, arg: u32) -> Result<u8, Error> {
        if command != 0 && command != 12 {
            self.ready(BUSY_US)?;
        }
        let mut frame = [0x40 | command, 0, 0, 0, 0, 0];
        frame[1..5].copy_from_slice(&arg.to_be_bytes());
        frame[5] = crc7(&frame[..5]) | 1;
        self.write(&frame)?;
        if command == 12 {
            // A byte of the stopped read still comes.
            self.byte()?;
        }
        // The answer within 8 bytes.
        for _ in 0..10 {
            let answer = self.byte()?;
            if answer & 0x80 == 0 {
                return Ok(answer);
            }
        }
        Err(Error::Timeout)
    }

    /// A command the card must answer with 0.
    fn expect(&mut self, command: u8, arg: u32) -> Result<(), Error> {
        match self.command(command, arg)? {
            0 => Ok(()),
            answer => Err(Error::Command(command, answer)),
        }
    }

    /// A block of data from the card into `buf` (8 bytes at least): polled
    /// for 8 bytes at a time, the first of the data among them kept.
    fn data(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        let mut chunk = [0xFF; 8];
        self.start_wait();
        let start = loop {
            self.transfer(&mut chunk)?;
            if let Some(at) = chunk.iter().position(|&b| b != 0xFF) {
                break at;
            }
            self.wait(READ_US)?;
        };
        if chunk[start] != DATA_TOKEN {
            return Err(Error::Data);
        }
        let head = &chunk[start + 1..];
        buf[..head.len()].copy_from_slice(head);
        self.transfer(&mut buf[head.len()..])?;
        // Its checksum, unchecked (the card checks none either).
        self.transfer(&mut [0xFF; 2])
    }
}

impl<'a, BUS: SpiBus, CS: OutputPin, D: DelayNs> Card<'a, BUS, CS, D> {
    /// Starts the card, with the bus at most 400 kHz (faster after this)
    /// and the card given 80 clocks unselected before.
    pub fn new(bus: &'a RefCell<BUS>, cs: CS, delay: D) -> Result<Self, Error> {
        let mut card = Self {
            bus,
            cs,
            delay,
            high_capacity: false,
            blocks: 0,
        };
        card.cs.set_high().map_err(|_| Error::Pin)?;
        // Into SPI mode, idle.
        let mut idle = false;
        for _ in 0..10 {
            if card.with(|s| s.command(0, 0)) == Ok(IDLE) {
                idle = true;
                break;
            }
        }
        if !idle {
            return Err(Error::NoCard);
        }
        // A card of version 2 (all larger than 2 GB) echoes the pattern
        // for the voltage asked.
        let version_2 = card.with(|s| {
            if s.command(8, 0x1AA)? & ILLEGAL != 0 {
                return Ok(false);
            }
            let mut echo = [0; 4];
            s.transfer(&mut echo)?;
            match echo[2..] {
                [0x01, 0xAA] => Ok(true),
                _ => Err(Error::Unsupported),
            }
        })?;
        // Started: asked until it is no longer idle.
        let arg = if version_2 { 1 << 30 } else { 0 };
        let mut waited = 0;
        loop {
            match card.with(|s| {
                s.command(55, 0)?;
                s.command(41, arg)
            })? {
                0 => break,
                IDLE if waited < START_US => {
                    card.delay.delay_ms(10);
                    waited += 10_000;
                }
                IDLE => return Err(Error::Timeout),
                answer => return Err(Error::Command(41, answer)),
            }
        }
        if version_2 {
            card.high_capacity = card.with(|s| {
                s.expect(58, 0)?;
                let mut ocr = [0; 4];
                s.transfer(&mut ocr)?;
                Ok(ocr[0] & 0x40 != 0)
            })?;
        }
        if !card.high_capacity {
            card.with(|s| s.expect(16, BLOCK as u32))?;
        }
        let mut csd = [0; 16];
        card.with(|s| {
            s.expect(9, 0)?;
            s.data(&mut csd)
        })?;
        card.blocks = blocks(&csd).ok_or(Error::Unsupported)?;
        Ok(card)
    }

    /// Blocks on the card.
    pub fn blocks(&self) -> u32 {
        self.blocks
    }

    /// Reads blocks from `index` on, several in one command.
    pub fn read(&mut self, index: u32, blocks: &mut [[u8; BLOCK]]) -> Result<(), Error> {
        let address = self.address(index);
        match blocks {
            [] => Ok(()),
            [block] => self.with(|s| {
                s.expect(17, address)?;
                s.data(block)
            }),
            blocks => self.with(|s| {
                s.expect(18, address)?;
                let read = blocks.iter_mut().try_for_each(|block| s.data(block));
                // Stopped even after an error.
                s.command(12, 0)?;
                read?;
                s.ready(BUSY_US)
            }),
        }
    }

    /// Writes a block, waiting until the card has it.
    pub fn write(&mut self, index: u32, block: &[u8; BLOCK]) -> Result<(), Error> {
        let address = self.address(index);
        self.with(|s| {
            s.expect(24, address)?;
            s.write(&[0xFF, DATA_TOKEN])?;
            s.write(block)?;
            // A checksum, unchecked.
            s.write(&[0xFF, 0xFF])?;
            if s.byte()? & 0x1F != ACCEPTED {
                return Err(Error::Data);
            }
            // Written before the card is let go: power may go next.
            s.ready(BUSY_US)
        })
    }

    fn address(&self, index: u32) -> u32 {
        if self.high_capacity {
            index
        } else {
            index.saturating_mul(BLOCK as u32)
        }
    }

    /// Runs `f` with the card selected and the bus held.
    fn with<T>(
        &mut self,
        f: impl FnOnce(&mut Session<'_, BUS, D>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut bus = self.bus.try_borrow_mut().map_err(|_| Error::Spi)?;
        self.cs.set_low().map_err(|_| Error::Pin)?;
        let result = f(&mut Session {
            bus: &mut bus,
            delay: &mut self.delay,
            tries: 0,
            waited: 0,
        });
        let flushed = bus.flush().map_err(|_| Error::Spi);
        let released = self.cs.set_high().map_err(|_| Error::Pin);
        // Eight clocks for the card to let go of its data line.
        let _ = bus.write(&[0xFF]);
        let value = result?;
        flushed?;
        released?;
        Ok(value)
    }
}

/// The blocks of a card from its CSD register (version 1 or 2).
fn blocks(csd: &[u8; 16]) -> Option<u32> {
    match csd[0] >> 6 {
        0 => {
            let read_bits = u32::from(csd[5] & 0x0F);
            let size = (u32::from(csd[6] & 0x03) << 10)
                | (u32::from(csd[7]) << 2)
                | (u32::from(csd[8]) >> 6);
            let mult = (u32::from(csd[9] & 0x03) << 1) | (u32::from(csd[10]) >> 7);
            let shift = (mult + 2 + read_bits).checked_sub(9)?;
            (size + 1).checked_shl(shift)
        }
        1 => {
            let size =
                (u32::from(csd[7] & 0x3F) << 16) | (u32::from(csd[8]) << 8) | u32::from(csd[9]);
            (size + 1).checked_mul(1024)
        }
        _ => None,
    }
}

/// The 7-bit checksum of a command (shifted left by one).
fn crc7(bytes: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &byte in bytes {
        let mut byte = byte;
        for _ in 0..8 {
            crc = if (byte ^ crc) & 0x80 != 0 {
                (crc << 1) ^ (0x09 << 1)
            } else {
                crc << 1
            };
            byte <<= 1;
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::VecDeque;
    use alloc::rc::Rc;
    use alloc::vec::Vec;
    use core::cell::Cell;
    use core::convert::Infallible;

    #[derive(PartialEq)]
    enum Mode {
        Commands,
        /// Blocks sent from this one on until stopped.
        Reading(u32),
        /// A block being received for this one, its bytes so far.
        Writing(u32, Option<Vec<u8>>),
    }

    /// A card, simulated byte by byte.
    struct FakeCard {
        selected: Rc<Cell<bool>>,
        high_capacity: bool,
        blocks: Vec<[u8; BLOCK]>,
        frame: Vec<u8>,
        out: VecDeque<u8>,
        mode: Mode,
        commands: Vec<u8>,
        /// Bytes on the bus.
        clocked: usize,
        /// Bus operations (transfers and writes).
        operations: usize,
        idle: bool,
        app: bool,
    }

    impl FakeCard {
        fn new(high_capacity: bool) -> (Self, Rc<Cell<bool>>) {
            let selected = Rc::new(Cell::new(false));
            let blocks = (0..64u32)
                .map(|n| core::array::from_fn(|i| (n as usize * 7 + i) as u8))
                .collect();
            let card = Self {
                selected: selected.clone(),
                high_capacity,
                blocks,
                frame: Vec::new(),
                out: VecDeque::new(),
                mode: Mode::Commands,
                commands: Vec::new(),
                clocked: 0,
                operations: 0,
                idle: false,
                app: false,
            };
            (card, selected)
        }

        fn index(&self, arg: u32) -> u32 {
            if self.high_capacity { arg } else { arg / 512 }
        }

        /// A block with a few bytes of latency before its token.
        fn send_block(&mut self, data: &[u8]) {
            self.out.extend([0xFF; 13]);
            self.out.push_back(DATA_TOKEN);
            self.out.extend(data);
            self.out.extend([0x12, 0x34]);
        }

        fn answer(&mut self, r1: u8) {
            self.out.push_back(0xFF);
            self.out.push_back(r1);
        }

        fn run(&mut self, command: u8, arg: u32) {
            self.commands.push(command);
            let app = core::mem::take(&mut self.app);
            let r1 = if self.idle { IDLE } else { 0 };
            match command {
                0 => {
                    self.idle = true;
                    self.answer(IDLE);
                }
                8 => {
                    self.answer(r1);
                    self.out.extend([0, 0, 0x01, arg as u8]);
                }
                55 => {
                    self.app = true;
                    self.answer(r1);
                }
                41 if app => {
                    self.idle = false;
                    self.answer(0);
                }
                58 => {
                    self.answer(r1);
                    let ccs = if self.high_capacity { 0x40 } else { 0 };
                    self.out.extend([0x80 | ccs, 0xFF, 0x80, 0]);
                }
                16 => self.answer(0),
                9 => {
                    self.answer(0);
                    // Version 2 with C_SIZE 0: 1024 blocks (the
                    // simulated card holds fewer).
                    let mut csd = [0u8; 16];
                    csd[0] = 0x40;
                    self.send_block(&csd);
                }
                17 => {
                    self.answer(0);
                    let block = self.blocks[self.index(arg) as usize];
                    self.send_block(&block);
                }
                18 => {
                    self.answer(0);
                    self.mode = Mode::Reading(self.index(arg));
                }
                12 => {
                    self.mode = Mode::Commands;
                    self.out.clear();
                    // The stuff byte, then the answer, then busy a while.
                    self.out.extend([0xAA, 0xFF, 0x00, 0x00, 0x00]);
                }
                24 => {
                    self.answer(0);
                    self.mode = Mode::Writing(self.index(arg), None);
                }
                _ => self.answer(ILLEGAL),
            }
        }

        fn exchange(&mut self, byte: u8) -> u8 {
            self.clocked += 1;
            if !self.selected.get() {
                self.frame.clear();
                return 0xFF;
            }
            if let Mode::Reading(next) = self.mode
                && self.out.is_empty()
            {
                let block = self.blocks[next as usize];
                self.send_block(&block);
                self.mode = Mode::Reading(next + 1);
            }
            let out = self.out.pop_front().unwrap_or(0xFF);
            if let Mode::Writing(index, data) = &mut self.mode {
                {
                    match data {
                        None if byte == DATA_TOKEN => *data = Some(Vec::new()),
                        None => {}
                        Some(bytes) => {
                            bytes.push(byte);
                            if bytes.len() == BLOCK + 2 {
                                let index = *index as usize;
                                self.blocks[index].copy_from_slice(&bytes[..BLOCK]);
                                self.mode = Mode::Commands;
                                // Taken, then busy writing.
                                self.out.extend([0xE5, 0, 0, 0]);
                            }
                        }
                    }
                    return out;
                }
            }
            if !self.frame.is_empty() || byte & 0xC0 == 0x40 {
                self.frame.push(byte);
                if self.frame.len() == 6 {
                    let frame = core::mem::take(&mut self.frame);
                    assert_eq!(crc7(&frame[..5]) | 1, frame[5], "checksum");
                    let arg = u32::from_be_bytes([frame[1], frame[2], frame[3], frame[4]]);
                    self.run(frame[0] & 0x3F, arg);
                }
            }
            out
        }
    }

    impl embedded_hal::spi::ErrorType for FakeCard {
        type Error = Infallible;
    }

    impl SpiBus for FakeCard {
        fn read(&mut self, words: &mut [u8]) -> Result<(), Infallible> {
            self.operations += 1;
            for w in words {
                *w = self.exchange(0xFF);
            }
            Ok(())
        }

        fn write(&mut self, words: &[u8]) -> Result<(), Infallible> {
            self.operations += 1;
            for &w in words {
                self.exchange(w);
            }
            Ok(())
        }

        fn transfer(&mut self, read: &mut [u8], write: &[u8]) -> Result<(), Infallible> {
            self.operations += 1;
            for i in 0..read.len().max(write.len()) {
                let byte = self.exchange(write.get(i).copied().unwrap_or(0xFF));
                if let Some(r) = read.get_mut(i) {
                    *r = byte;
                }
            }
            Ok(())
        }

        fn transfer_in_place(&mut self, words: &mut [u8]) -> Result<(), Infallible> {
            self.operations += 1;
            for w in words {
                *w = self.exchange(*w);
            }
            Ok(())
        }

        fn flush(&mut self) -> Result<(), Infallible> {
            Ok(())
        }
    }

    struct Select(Rc<Cell<bool>>);

    impl embedded_hal::digital::ErrorType for Select {
        type Error = Infallible;
    }

    impl OutputPin for Select {
        fn set_low(&mut self) -> Result<(), Infallible> {
            self.0.set(true);
            Ok(())
        }

        fn set_high(&mut self) -> Result<(), Infallible> {
            self.0.set(false);
            Ok(())
        }
    }

    struct NoDelay;

    impl DelayNs for NoDelay {
        fn delay_ns(&mut self, _: u32) {}
    }

    /// The commands a card is started with.
    fn started(high_capacity: bool) -> Vec<u8> {
        let (fake, selected) = FakeCard::new(high_capacity);
        let bus = RefCell::new(fake);
        let card = Card::new(&bus, Select(selected), NoDelay).unwrap();
        assert_eq!(card.blocks(), 1024);
        assert_eq!(card.high_capacity, high_capacity);
        drop(card);
        bus.into_inner().commands
    }

    #[test]
    fn started_as_cards_start() {
        assert_eq!(started(true), [0, 8, 55, 41, 58, 9]);
        // An old card's blocks are addressed by bytes, set to 512.
        assert_eq!(started(false), [0, 8, 55, 41, 58, 16, 9]);
    }

    #[test]
    fn blocks_read_one_or_several_at_a_time() {
        for high_capacity in [true, false] {
            let (fake, selected) = FakeCard::new(high_capacity);
            let want = fake.blocks.clone();
            let bus = RefCell::new(fake);
            let mut card = Card::new(&bus, Select(selected), NoDelay).unwrap();
            let mut one = [[0u8; BLOCK]];
            card.read(5, &mut one).unwrap();
            assert_eq!(one[0], want[5]);
            let operations = bus.borrow().operations;
            let mut four = [[0u8; BLOCK]; 4];
            card.read(9, &mut four).unwrap();
            assert_eq!(&four[..], &want[9..13]);
            // Not a transaction a byte: a few for a block, its latency
            // polled 8 bytes at a time.
            assert!(bus.borrow().operations - operations < 40);
            assert_eq!(
                bus.borrow().commands[bus.borrow().commands.len() - 3..],
                [17, 18, 12]
            );
            // And read on after the stop.
            card.read(1, &mut one).unwrap();
            assert_eq!(one[0], want[1]);
        }
    }

    #[test]
    fn blocks_written_and_read_back() {
        for high_capacity in [true, false] {
            let (fake, selected) = FakeCard::new(high_capacity);
            let bus = RefCell::new(fake);
            let mut card = Card::new(&bus, Select(selected), NoDelay).unwrap();
            let block: [u8; BLOCK] = core::array::from_fn(|i| (i * 3) as u8);
            card.write(20, &block).unwrap();
            assert_eq!(bus.borrow().blocks[20], block);
            let mut back = [[0u8; BLOCK]];
            card.read(20, &mut back).unwrap();
            assert_eq!(back[0], block);
        }
    }

    #[test]
    fn sizes_from_both_versions_of_the_register() {
        // Version 2: (C_SIZE + 1) × 512 KB; 15079 is an 8 GB card.
        let mut csd = [0u8; 16];
        csd[0] = 0x40;
        csd[7] = 0x00;
        csd[8] = 0x3A;
        csd[9] = 0xE7;
        assert_eq!(blocks(&csd), Some(15080 * 1024));
        // Version 1: (C_SIZE + 1) × 2^(C_SIZE_MULT + 2) × 2^READ_BL_LEN
        // bytes; a 1 GB card.
        let mut csd = [0u8; 16];
        csd[5] = 0x0A; // 1024-byte blocks
        csd[6] = 0x03; // C_SIZE 4095: its top two bits
        csd[7] = 0xFF;
        csd[8] = 0xC0;
        csd[9] = 0x03; // C_SIZE_MULT 7
        csd[10] = 0x80;
        assert_eq!(blocks(&csd), Some(4096 * 512 * 2));
        assert_eq!(blocks(&[0xC0; 16]), None);
    }

    #[test]
    fn command_checksums() {
        // As cards want them before they check no more.
        assert_eq!(crc7(&[0x40, 0, 0, 0, 0]) | 1, 0x95);
        assert_eq!(crc7(&[0x48, 0, 0, 0x01, 0xAA]) | 1, 0x87);
    }
}
