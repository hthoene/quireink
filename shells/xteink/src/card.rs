//! The microSD card: its blocks, read and written over SPI (by
//! `embedded-sdmmc`), for QuireInk's FAT storage.

use embedded_hal::delay::DelayNs;
use embedded_hal::spi::SpiDevice;
use embedded_sdmmc::{Block, BlockCount, BlockDevice, BlockIdx, SdCard};
use quireink_fat::{BLOCK, Blocks, DiskError};

/// The card, as blocks.
pub struct Card<SPI: SpiDevice, D: DelayNs>(pub SdCard<SPI, D>);

impl<SPI: SpiDevice, D: DelayNs> Blocks for Card<SPI, D> {
    fn read_block(&mut self, index: u32, block: &mut [u8; BLOCK]) -> Result<(), DiskError> {
        let mut blocks = [Block::new()];
        self.0
            .read(&mut blocks, BlockIdx(index))
            .map_err(|_| DiskError)?;
        block.copy_from_slice(&blocks[0].contents);
        Ok(())
    }

    fn write_block(&mut self, index: u32, block: &[u8; BLOCK]) -> Result<(), DiskError> {
        let mut blocks = [Block::new()];
        blocks[0].contents.copy_from_slice(block);
        self.0
            .write(&blocks, BlockIdx(index))
            .map_err(|_| DiskError)
    }

    fn blocks(&mut self) -> Result<u32, DiskError> {
        self.0
            .num_blocks()
            .map(|BlockCount(n)| n)
            .map_err(|_| DiskError)
    }
}
