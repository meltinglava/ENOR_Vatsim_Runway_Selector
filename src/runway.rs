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
