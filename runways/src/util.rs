pub fn diff_angle(a: u32, b: u32) -> u32 {
    let diff = (a as i32 - b as i32).abs();
    if diff > 180 {
        360 - diff as u32
    } else {
        diff as u32
    }
}

#[allow(dead_code)]
pub fn diff_rotation(a: u32, b: u32) -> u32 {
    let diff = a as i32 - b as i32;
    if diff < 0 {
        (diff + 360) as u32
    } else {
        diff as u32
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
