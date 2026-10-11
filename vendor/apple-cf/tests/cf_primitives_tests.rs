use apple_cf::cf::{
    AsCFType, CFData, CFDate, CFError as CoreFoundationError, CFNumber, CFString, CFType, CFUUID,
};
use apple_cf::raw;
use std::collections::HashSet;
use std::time::UNIX_EPOCH;

#[test]
fn cf_primitives_round_trip() {
    let string = CFString::new("apple-cf");
    let number = CFNumber::from_f64(3.5);
    let bytes = CFData::from_bytes([1_u8, 2, 3]);
    let now = CFDate::now();
    let uuid = CFUUID::new();
    let error = CoreFoundationError::new(
        &CFString::new("com.doomfish.apple-cf.tests"),
        5,
        Some("failure"),
    );

    assert_eq!(string.to_string(), "apple-cf");
    assert_eq!(number.to_f64(), Some(3.5));
    assert!(number.is_float_type());
    assert_eq!(bytes.to_vec(), vec![1, 2, 3]);
    assert!(now.to_system_time().is_some());
    assert_eq!(uuid.bytes().len(), 16);
    assert_eq!(error.code(), 5);
    assert_eq!(error.domain().to_string(), "com.doomfish.apple-cf.tests");
}

#[test]
fn cf_string_keeps_interior_nul_and_counts_utf16_units() {
    let with_nul = CFString::new("a\0b");
    assert_eq!(with_nul.len(), 3);
    assert_eq!(with_nul.to_string_lossy(), "a\0b");

    let emoji = CFString::new("😀");
    assert_eq!(emoji.len(), 2);
    assert_eq!(emoji.to_string(), "😀");
    assert!(CFString::new("").is_empty());
    assert_eq!(CFString::new("").to_string_lossy(), "");
}

#[test]
fn cf_string_lossy_conversion_replaces_unpaired_surrogates() {
    let units = [0x61_u16, 0xD800, 0x62];
    let ptr = unsafe { raw::CFStringCreateWithCharacters(std::ptr::null(), units.as_ptr(), 3) };
    let string = unsafe { CFString::from_raw(ptr.cast_mut().cast()) }.expect("string");
    assert_eq!(string.len(), 3);
    assert_eq!(string.to_string_lossy(), "a\u{FFFD}b");
}

#[test]
fn cf_string_bridge_c_strings_use_the_matching_allocator() {
    let string = CFString::new("apple-cf");
    let copied = unsafe {
        apple_cf::utils::ffi_string::ffi_string_owned(|| {
            apple_cf::ffi::cf_string_copy_cstring(string.as_ptr())
        })
    };
    assert_eq!(copied.as_deref(), Some("apple-cf"));
}

#[test]
fn cf_uuid_parse_accepts_only_well_formed_uuids() {
    let canonical = "68753A44-4D6F-1226-9C60-0050E4C00067";
    let parsed = CFUUID::parse_str(canonical).expect("canonical uuid");
    assert_eq!(parsed.to_string(), canonical);
    assert_eq!(
        CFUUID::parse_str("{68753A44-4D6F-1226-9C60-0050E4C00067}").map(|uuid| uuid.bytes()),
        Some(parsed.bytes())
    );
    assert_eq!(
        CFUUID::parse_str("68753a44-4d6f-1226-9c60-0050e4c00067").map(|uuid| uuid.bytes()),
        Some(parsed.bytes())
    );
    for malformed in [
        "",
        "not-a-uuid",
        "68753A44-4D6F",
        "68753A444D6F12269C600050E4C00067",
        "68753A44-4D6F-1226-9C60-0050E4C0006\0",
        "68753A44-4D6F-1226-9C60-0050E4C0006G",
        "{68753A44-4D6F-1226-9C60-0050E4C00067",
    ] {
        assert!(CFUUID::parse_str(malformed).is_none(), "{malformed:?}");
    }
}

#[test]
fn cf_date_to_system_time_rejects_unrepresentable_dates() {
    for absolute_time in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e19, 1e300, -1e300] {
        assert!(
            CFDate::from_absolute_time(absolute_time)
                .to_system_time()
                .is_none(),
            "{absolute_time}"
        );
    }
    let before_epoch = CFDate::from_absolute_time(-978_307_201.0)
        .to_system_time()
        .expect("date before 1970");
    assert!(before_epoch < UNIX_EPOCH);
}

#[test]
fn cf_number_integer_conversions_are_exact() {
    let max = CFNumber::from_u64(u64::MAX);
    assert_eq!(max.to_u64(), Some(u64::MAX));
    assert_eq!(max.to_i64(), None);
    assert!(!max.is_float_type());
    assert_eq!(CFNumber::from_u64(7).to_i64(), Some(7));
    assert_eq!(CFNumber::from_i64(-1).to_u64(), None);
    assert_eq!(
        CFNumber::from_i64(i64::MAX).to_u64(),
        Some(u64::try_from(i64::MAX).expect("i64::MAX fits in u64"))
    );
    assert_eq!(CFNumber::from_f64(1e19).to_i64(), None);
    assert_eq!(
        CFNumber::from_f64(1e19).to_u64(),
        Some(10_000_000_000_000_000_000)
    );
    assert_eq!(CFNumber::from_f64(1.5).to_i64(), None);
    assert_eq!(CFNumber::from_f64(-3.0).to_i64(), Some(-3));
    assert_eq!(CFNumber::from_f64(-3.0).to_u64(), None);
}

#[test]
fn cf_type_hashing_accepts_every_cf_hash_value() {
    let numbers: HashSet<CFNumber> = (1..=500_i32)
        .map(|index| CFNumber::from_f64(-f64::from(index) - 0.5))
        .collect();
    assert_eq!(numbers.len(), 500);
    let hashes: Vec<usize> = numbers
        .iter()
        .map(|number| number.to_cf_type().hash_code())
        .collect();
    assert!(hashes.iter().any(|&hash| hash > usize::MAX / 2));
    let erased: HashSet<CFType> = numbers.iter().map(AsCFType::to_cf_type).collect();
    assert_eq!(erased.len(), 500);
    let strings: HashSet<CFString> = (0..500)
        .map(|index| CFString::new(&format!("k{index}")))
        .collect();
    assert_eq!(strings.len(), 500);
}

#[test]
fn cf_error_description_may_contain_nul() {
    let error = CoreFoundationError::new(
        &CFString::new("com.doomfish.apple-cf.tests"),
        7,
        Some("bad\0input"),
    );
    assert_eq!(error.code(), 7);
    let description = error.description_string().expect("description");
    assert!(description.to_string_lossy().starts_with("bad"));
}

#[test]
fn cf_data_round_trips_every_byte() {
    let bytes: Vec<u8> = (0..=255).collect();
    assert_eq!(CFData::from_bytes(&bytes).to_vec(), bytes);
    assert!(CFData::from_bytes([]).to_vec().is_empty());
}
