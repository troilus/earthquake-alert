use country_boundaries::{BOUNDARIES_ODBL_60X30, CountryBoundaries, LatLon};
use std::sync::OnceLock;

static BOUNDARIES: OnceLock<Option<CountryBoundaries>> = OnceLock::new();

/// 根据离线国界数据判断坐标是否位于中国境内。
pub fn is_in_china(latitude: f64, longitude: f64) -> bool {
    let Some(point) = LatLon::new(latitude, longitude).ok() else {
        return false;
    };
    BOUNDARIES
        .get_or_init(|| CountryBoundaries::from_reader(BOUNDARIES_ODBL_60X30).ok())
        .as_ref()
        .is_some_and(|boundaries| boundaries.is_in(point, "CN"))
}

#[cfg(test)]
mod tests {
    use super::is_in_china;

    #[test]
    fn identifies_points_inside_and_outside_china() {
        assert!(is_in_china(39.9042, 116.4074));
        assert!(is_in_china(30.5728, 104.0668));
        assert!(!is_in_china(35.6762, 139.6503));
        assert!(!is_in_china(-22.75, 171.63));
    }
}
