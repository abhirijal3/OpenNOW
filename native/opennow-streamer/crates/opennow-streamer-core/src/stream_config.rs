#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaVideoCodec {
    H264,
    H265,
    Av1,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum MediaColorQuality {
    #[default]
    EightBit420,
    EightBit444,
    TenBit420,
    TenBit444,
}

impl MediaColorQuality {
    pub const fn protocol_name(self) -> &'static str {
        match self {
            Self::EightBit420 => "8bit_420",
            Self::EightBit444 => "8bit_444",
            Self::TenBit420 => "10bit_420",
            Self::TenBit444 => "10bit_444",
        }
    }

    pub const fn bit_depth(self) -> u8 {
        match self {
            Self::EightBit420 | Self::EightBit444 => 8,
            Self::TenBit420 | Self::TenBit444 => 10,
        }
    }

    pub const fn is_444(self) -> bool {
        matches!(self, Self::EightBit444 | Self::TenBit444)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaStreamConfig {
    pub codec: MediaVideoCodec,
    /// Color class accepted by CloudMatch and requested again during NVST setup.
    pub color_quality: MediaColorQuality,
    pub hdr: bool,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
}

impl Default for MediaStreamConfig {
    fn default() -> Self {
        Self {
            codec: MediaVideoCodec::H264,
            color_quality: MediaColorQuality::default(),
            hdr: false,
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_bps: 75_000_000,
        }
    }
}
