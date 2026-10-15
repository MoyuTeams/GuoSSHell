use std::alloc::Layout;
use std::path::PathBuf;

use super::{
    NumericReject, PublicLayouts, add, align_up, array, btree_nodes, extend, mul, struct_upper,
};

#[test]
fn checked_layout_matches_real_arrays_and_rejects_overflow() -> Result<(), NumericReject> {
    for n in [0, 1, 7, 19, 112, 128, 65536] {
        assert_eq!(
            array(Layout::new::<u64>(), n)?,
            Layout::array::<u64>(n).map_err(|_| NumericReject::InvalidLayout)?
        );
        let (actual, offset) = extend(Layout::new::<u8>(), array(Layout::new::<u64>(), n)?)?;
        assert_eq!(offset, 8);
        assert_eq!(actual.size(), align_up(add(8, mul(8, n)?)?, 8)?);
    }
    assert!(array(Layout::new::<u64>(), usize::MAX).is_err());
    assert!(array(Layout::new::<u8>(), usize::MAX).is_err());
    assert_eq!(add(usize::MAX, 1), Err(NumericReject::Overflow));
    assert_eq!(mul(usize::MAX, 2), Err(NumericReject::Overflow));
    assert_eq!(align_up(usize::MAX, 8), Err(NumericReject::Overflow));
    assert_eq!(align_up(12, 3), Err(NumericReject::InvalidLayout));
    Ok(())
}

#[test]
fn actual_public_and_private_field_permutation_bounds() -> Result<(), NumericReject> {
    let public = PublicLayouts::actual();
    assert_eq!(public.string, Layout::new::<String>());
    assert_eq!(public.path, Layout::new::<PathBuf>());
    assert_eq!(public.value, Layout::new::<serde_json::Value>());
    let string_nodes = btree_nodes(public.string, public.string, 1)?;
    assert_eq!(
        (string_nodes.leaf.size(), string_nodes.internal.size()),
        (552, 648)
    );
    let value_nodes = btree_nodes(public.string, public.value, 1)?;
    assert_eq!(
        (value_nodes.leaf.size(), value_nodes.internal.size()),
        (640, 736)
    );
    let set_nodes = btree_nodes(public.string, Layout::new::<()>(), 1)?;
    assert_eq!(
        (set_nodes.leaf.size(), set_nodes.internal.size()),
        (288, 384)
    );
    let permuted = struct_upper(
        [
            Layout::new::<u8>(),
            Layout::new::<u64>(),
            Layout::new::<u16>(),
        ],
        1,
    )?;
    assert_eq!(permuted.size(), 24);
    let zero_aligned = Layout::from_size_align(0, 64).map_err(|_| NumericReject::InvalidLayout)?;
    assert_eq!(struct_upper([zero_aligned], 1)?, zero_aligned);
    assert!(struct_upper([Layout::new::<u8>()], 3).is_err());
    Ok(())
}

#[test]
fn json_string_path_move_preserves_real_backing_capacity() {
    let mut text = String::with_capacity(71);
    text.push_str("\u{1f642}\0\u{4e2d}");
    let pointer = text.as_ptr();
    let capacity = text.capacity();
    let path = PathBuf::from(text);
    assert_eq!(path.as_os_str().as_encoded_bytes().as_ptr(), pointer);
    assert_eq!(path.capacity(), capacity);
}
