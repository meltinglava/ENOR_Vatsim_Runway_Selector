#[derive(Debug)]
pub struct RunwayDirection {
    pub degrees: u16,
    pub identifier: String,
}

#[derive(Debug)]
pub struct Runway {
    pub runways: [RunwayDirection; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunwayUse {
    Departing,
    Arriving,
    Both,
}

impl RunwayUse {
    pub fn merged_with(self, other: Self) -> Self {
        match (self, other) {
            (Self::Both, _) | (_, Self::Both) => Self::Both,
            (Self::Arriving, Self::Departing) | (Self::Departing, Self::Arriving) => Self::Both,
            (existing, _) => existing,
        }
    }

    pub fn report_suffix(self) -> &'static str {
        match self {
            Self::Arriving => " Arr",
            Self::Departing => " Dep",
            Self::Both => "",
        }
    }

    pub fn active_runway_flags(self) -> &'static [u8] {
        match self {
            Self::Departing => &[1],
            Self::Arriving => &[0],
            Self::Both => &[1, 0],
        }
    }
}
