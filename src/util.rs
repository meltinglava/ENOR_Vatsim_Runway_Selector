pub fn diff_angle(a: u16, b: u16) -> u16 {
    let diff = (a as i16 - b as i16).abs();
    if diff > 180 {
        360 - diff as u16
    } else {
        diff as u16
    }
}

#[allow(dead_code)]
pub fn diff_rotation(a: u16, b: u16) -> u16 {
    let diff = a as i16 - b as i16;
    if diff < 0 {
        (diff + 360) as u16
    } else {
        diff as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn test_diff_angle() {
        assert_eq!(diff_angle(10, 350), 20);
        assert_eq!(diff_angle(0, 180), 180);
        assert_eq!(diff_angle(270, 90), 180);
        assert_eq!(diff_angle(90, 270), 180);
        assert_eq!(diff_angle(0, 0), 0);
    }

    #[test]
    fn test_diff_rotation() {
        assert_eq!(diff_rotation(10, 350), 20);
        assert_eq!(diff_rotation(0, 180), 180);
        assert_eq!(diff_rotation(270, 90), 180);
        assert_eq!(diff_rotation(90, 270), 180);
        assert_eq!(diff_rotation(0, 0), 0);
    }
}
