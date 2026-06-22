// Flag Validator Contract & Boundary Tests
//
// Hardens the domain validation logic against impossible flag combinations
// and verifies adherence to the single recommended combination standard.

use crate::flag_validator::{
    FlagBase, FlagMode, FlagRequest, FlagTier, FlagValidation, validate_flags,
    validate_flags_result_with_ultimate, validate_flags_with_ultimate,
};

#[test]
fn test_flag_contract_rejects_partial_combinations_loudly() {
    let requests = [
        (true, true, false),
        (true, false, true),
        (false, true, true),
        (false, false, false),
    ];

    for (explore, match_quality, compress) in requests {
        let validation = validate_flags(explore, match_quality, compress);
        assert!(
            matches!(validation, FlagValidation::Invalid(_)),
            "Expected Invalid for partial flag combination ({explore}, {match_quality}, {compress})"
        );
        let FlagValidation::Invalid(err_msg) = validation else {
            unreachable!("Validation must be Invalid for partial combinations")
        };
        assert!(err_msg.contains("Only the recommended flag combination is supported"));
    }
}

#[test]
fn test_flag_contract_ultimate_requires_full_base() {
    // Ultimate explore should fail if base flags are turned off
    let invalid_req = FlagRequest {
        base: FlagBase {
            explore: true,
            match_quality: false,
            compress: true,
        },
        tier: FlagTier { ultimate: true },
    };

    let validation = validate_flags_with_ultimate(invalid_req);
    assert!(
        matches!(validation, FlagValidation::Invalid(_)),
        "Ultimate mode must be rejected if base flags are incomplete"
    );
}

#[test]
fn test_flag_contract_valid_modes_maintain_correct_properties() {
    let precise_req = FlagRequest {
        base: FlagBase {
            explore: true,
            match_quality: true,
            compress: true,
        },
        tier: FlagTier { ultimate: false },
    };
    let ultimate_req = FlagRequest {
        base: FlagBase {
            explore: true,
            match_quality: true,
            compress: true,
        },
        tier: FlagTier { ultimate: true },
    };

    assert_eq!(
        validate_flags_result_with_ultimate(precise_req).unwrap(),
        FlagMode::PreciseQualityWithCompress
    );
    assert_eq!(
        validate_flags_result_with_ultimate(ultimate_req).unwrap(),
        FlagMode::UltimateExplore
    );
}
