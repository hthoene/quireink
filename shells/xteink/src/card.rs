//! The microSD card's blocks (read and written by [`quireink_x4::card`])
//! for QuireInk's FAT storage.

use embedded_hal::delay::DelayNs;
use embedded_hal::digital::OutputPin;
use embedded_hal::spi::SpiBus;
use quireink_fat::{BLOCK, Blocks, DiskError};

/// The card, as blocks.
pub struct Card<'a, BUS, CS, D>(pub quireink_x4::card::Card<'a, BUS, CS, D>);

impl<BUS: SpiBus, CS: OutputPin, D: DelayNs> Blocks for Card<'_, BUS, CS, D> {
    fn read_block(&mut self, index: u32, block: &mut [u8; BLOCK]) -> Result<(), DiskError> {
        self.read_blocks(index, block)
    }

    fn write_block(&mut self, index: u32, block: &[u8; BLOCK]) -> Result<(), DiskError> {
        self.write_blocks(index, block)
    }

    fn read_blocks(&mut self, index: u32, data: &mut [u8]) -> Result<(), DiskError> {
        let blocks = data.as_chunks_mut::<BLOCK>().0;
        crate::traced::blocks(index, blocks.len() as u32, 0);
        self.0.read(index, blocks).map_err(|_| DiskError)
    }

    fn write_blocks(&mut self, index: u32, data: &[u8]) -> Result<(), DiskError> {
        let blocks = data.as_chunks::<BLOCK>().0;
        crate::traced::blocks(index, 0, blocks.len() as u32);
        self.0.write(index, blocks).map_err(|_| DiskError)
    }

    fn blocks(&mut self) -> Result<u32, DiskError> {
        Ok(self.0.blocks())
    }
}
