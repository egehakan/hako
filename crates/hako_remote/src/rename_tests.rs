use super::*;
use crate::protocol::ErrorCode;

#[test]
fn keeps_a_trimmed_name_within_the_limit_or_none() {
    assert_eq!(chosen_name(None, 5), Ok(None));
    assert_eq!(
        chosen_name(Some("  build \n"), 5),
        Ok(Some("build".to_owned()))
    );
    assert_eq!(chosen_name(Some("ççççç"), 5), Ok(Some("ççççç".to_owned())));
    for refused in ["", "   ", "toolong"] {
        assert_eq!(
            chosen_name(Some(refused), 5).unwrap_err().code,
            ErrorCode::BadRequest
        );
    }
}

#[test]
fn takes_only_tab_ids_of_digits() {
    assert_eq!(well_formed_tab_id("1234"), Ok(()));
    for refused in ["?", "", "12a", "-1"] {
        assert_eq!(
            well_formed_tab_id(refused).unwrap_err().code,
            ErrorCode::BadRequest
        );
    }
}
