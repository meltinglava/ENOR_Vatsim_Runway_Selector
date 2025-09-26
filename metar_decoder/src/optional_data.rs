use std::fmt::Display;

use nom::{
    AsChar, Input, Parser,
    branch::{Choice, alt},
    character::complete::char,
    combinator::{map, value},
    error::ParseError,
    multi::count,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionalData<T, const N: usize> {
    Undefined,
    Data(T),
}

pub type OptionalNumber<const N: usize> = OptionalData<u32, N>;

impl<T, const N: usize> OptionalData<T, N> {
    pub fn new(data: T) -> Self {
        OptionalData::Data(data)
    }

    pub fn to_option(self) -> Option<T> {
        self.into()
    }
}

impl<T: Clone, const N: usize> OptionalData<T, N> {
    // Ugly type signature. Should be able to return impl Parser....
    pub fn optional_field<P, I, E: ParseError<I>>(
        p: P,
    ) -> Choice<(
        impl Parser<I, Output = OptionalData<T, N>, Error = E>,
        impl Parser<I, Output = OptionalData<T, N>, Error = E>,
    )>
    where
        P: Parser<I, Output = T, Error = E>,
        I: Input,
        <I as Input>::Item: AsChar,
    {
        alt((
            value(OptionalData::Undefined, count(char('/'), N)),
            map(p, OptionalData::Data),
        ))
    }
}

impl<T, const N: usize> Display for OptionalData<T, N>
where
    T: Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OptionalData::Undefined => write!(f, "{:/<N$}", ""),
            OptionalData::Data(data) => write!(f, "{:0N$}", data),
        }
    }
}

impl <const N: usize, T> From<OptionalData<T, N>> for Option<T> {
    fn from(value: OptionalData<T, N>) -> Self {
        match value {
            OptionalData::Undefined => None,
            OptionalData::Data(data) => Some(data),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format() {
        let data = OptionalData::<_, 3>::new(42);
        assert_eq!(format!("{:03}", data), "042");

        let undefined: OptionalData<u32, 3> = OptionalData::Undefined;
        assert_eq!(format!("{:3}", undefined), "///");
    }
}
