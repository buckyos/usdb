use ordinals::SatPoint;

pub struct Util {}

impl Util {
    pub fn zero_satpoint() -> SatPoint {
        const VALUE: &str = "0000000000000000000000000000000000000000000000000000000000000000:0:0";

        VALUE.parse::<SatPoint>().unwrap()
    }

    pub fn is_zero_satpoint(satpoint: &SatPoint) -> bool {
        satpoint == &Self::zero_satpoint()
    }
}
