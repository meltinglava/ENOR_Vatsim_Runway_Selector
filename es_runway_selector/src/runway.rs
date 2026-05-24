#[derive(Debug)]
pub struct RunwayDirection {
    pub identifier: String,
    pub degrees: u16,
}

#[derive(Debug)]
pub struct Runway {
    pub primary: RunwayDirection,
    pub reciprocal: Option<RunwayDirection>,
}

impl Runway {
    /// Iterate over whichever directions are present (1 or 2).
    pub fn iter(&self) -> impl Iterator<Item = &RunwayDirection> {
        std::iter::once(&self.primary).chain(self.reciprocal.iter())
    }
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
