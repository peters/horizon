/// Output orientation; portrait does not rotate the TV physically.
#[derive(Clone, Copy, Debug, Default)]
pub enum Orientation {
    #[default]
    Landscape,
    Portrait,
}
/// Explicit H.264 canvas size. Source detail is limited by the host's capture.
#[derive(Clone, Copy, Debug, Default)]
pub enum Resolution {
    Hd720,
    #[default]
    FullHd1080,
    Uhd4k,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct VideoFormat {
    pub orientation: Orientation,
    pub resolution: Resolution,
}
impl VideoFormat {
    #[must_use]
    pub const fn dimensions(self) -> (u16, u16) {
        let landscape = match self.resolution {
            Resolution::Hd720 => (1280, 720),
            Resolution::FullHd1080 => (1920, 1080),
            Resolution::Uhd4k => (3840, 2160),
        };
        match self.orientation {
            Orientation::Landscape => landscape,
            Orientation::Portrait => (landscape.1, landscape.0),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn supported_canvases_and_default_match_the_wire_format() {
        assert_eq!(VideoFormat::default().dimensions(), (1920, 1080));
        for (resolution, expected) in [
            (Resolution::Hd720, (1280, 720)),
            (Resolution::FullHd1080, (1920, 1080)),
            (Resolution::Uhd4k, (3840, 2160)),
        ] {
            assert_eq!(
                VideoFormat {
                    orientation: Orientation::Landscape,
                    resolution
                }
                .dimensions(),
                expected
            );
            assert_eq!(
                VideoFormat {
                    orientation: Orientation::Portrait,
                    resolution
                }
                .dimensions(),
                (expected.1, expected.0)
            );
        }
    }
}
