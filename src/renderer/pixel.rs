pub(crate) fn rgba8_alpha_to_r8(rgba8: &[u8]) -> Vec<u8> {
    rgba8.chunks_exact(4).map(|pixel| pixel[3]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_alpha_from_rgba8_pixels() {
        assert_eq!(rgba8_alpha_to_r8(&[1, 2, 3, 4, 5, 6, 7, 8]), vec![4, 8]);
    }

    #[test]
    fn extracts_no_pixels_from_empty_or_incomplete_input() {
        assert!(rgba8_alpha_to_r8(&[]).is_empty());
        assert!(rgba8_alpha_to_r8(&[1, 2, 3]).is_empty());
    }
}
