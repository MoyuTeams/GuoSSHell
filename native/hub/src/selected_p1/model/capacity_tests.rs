use std::alloc::Layout;
use std::collections::BTreeMap;

use super::{
    Growth, VecPeak, cautious, completed_nodes, height_upper, insert_peak_nodes, maximum_length,
    min_non_zero_cap, tree_clone_backing, tree_insert_peak,
};
use crate::audit::AuditScope;
use crate::selected_p1::model::layout::{NumericReject, btree_nodes};
use crate::selected_p1::resource::ledger::{
    Admission, Amount, CLASS_COUNT, ProcessLedger, ResourceClass,
};

fn verify_push<T: Default>(n: usize) -> Result<(), NumericReject> {
    let mut modeled = VecPeak::new(Layout::new::<T>(), 0)?;
    let mut actual = Vec::<T>::new();
    let generic = maximum_length(Layout::new::<T>(), 0, n)?;
    for _ in 0..n {
        modeled.append(1, Growth::Amortized)?;
        actual.push(T::default());
        assert_eq!(modeled.capacity, actual.capacity());
        assert!(modeled.peak_bytes <= generic.peak_bytes);
    }
    Ok(())
}

#[derive(Clone)]
struct Large {
    bytes: [u8; 1025],
}
impl Default for Large {
    fn default() -> Self {
        Self { bytes: [0; 1025] }
    }
}

#[test]
fn all_rawvec_growth_branches_and_zst_match_actual_std() -> Result<(), NumericReject> {
    assert_eq!(
        (
            min_non_zero_cap(1),
            min_non_zero_cap(2),
            min_non_zero_cap(1025)
        ),
        (8, 4, 1)
    );
    verify_push::<u8>(4096)?;
    verify_push::<u16>(4096)?;
    verify_push::<Large>(256)?;
    verify_push::<()>(4096)?;
    assert_eq!(Large::default().bytes[0], 0);
    let mut exact = VecPeak::new(Layout::new::<u8>(), 0)?;
    exact.append(3, Growth::Exact)?;
    exact.append(5, Growth::Exact)?;
    assert_eq!((exact.capacity, exact.peak_bytes), (8, 11));
    assert_eq!(cautious(Layout::new::<u64>(), usize::MAX), 131072);
    assert_eq!(cautious(Layout::new::<()>(), 12), 12);
    let before = exact;
    assert!(exact.append(usize::MAX, Growth::Amortized).is_err());
    assert_eq!(exact, before);
    assert!(maximum_length(Layout::new::<u64>(), 0, usize::MAX).is_err());
    Ok(())
}

#[test]
fn initial_clone_backing_and_arbitrary_append_blocks_are_covered() -> Result<(), NumericReject> {
    for initial in 0..17 {
        for n in initial..96 {
            let upper = maximum_length(Layout::new::<u64>(), initial, n)?;
            for block in 1..13 {
                let mut state = VecPeak::new(Layout::new::<u64>(), initial)?;
                while state.len < n {
                    state.append(block.min(n - state.len), Growth::Amortized)?;
                }
                assert!(state.capacity <= upper.capacity);
                assert!(state.peak_bytes <= upper.peak_bytes);
            }
        }
    }
    Ok(())
}

#[test]
fn node_split_and_clone_source_bounds_cover_actual_allocations() -> Result<(), NumericReject> {
    assert_eq!(completed_nodes(0)?, 0);
    assert_eq!(completed_nodes(12)?, 3);
    assert_eq!(height_upper(10), 0);
    assert_eq!(height_upper(11), 1);
    assert_eq!(height_upper(71), 2);
    assert_eq!(insert_peak_nodes(0)?, 0);
    assert_eq!(insert_peak_nodes(11)?, 1);
    assert_eq!(insert_peak_nodes(12)?, 6);
    let nodes = btree_nodes(Layout::new::<usize>(), Layout::new::<usize>(), 1)?;
    let n = 600;
    let peak = tree_insert_peak(nodes, n)?;
    let clone = tree_clone_backing(nodes, n)?;
    let root = ProcessLedger::new(peak + clone, [u32::MAX; CLASS_COUNT]);
    let grant = root.try_reserve(
        Amount {
            rust_bytes: peak + clone,
            c_bytes: 0,
            slots: 1,
        },
        ResourceClass::Decode,
        Admission {
            epoch: 1,
            source: None,
        },
    )?;
    let audit = AuditScope::start(&root);
    let mut map = BTreeMap::new();
    for value in 0..n {
        map.insert(value, value);
    }
    let copied = map.clone();
    assert_eq!(copied.len(), n);
    drop(copied);
    drop(map);
    let report = audit.finish();
    assert!(!report.overflow);
    assert!(
        report
            .events()
            .iter()
            .all(|event| event.size <= nodes.internal.size() && event.held_rust == peak + clone)
    );
    let mut live = 0;
    for event in report.events() {
        match event.kind {
            1 => live += event.size,
            2 => live -= event.size,
            _ => panic!("1"),
        }
        assert!(live <= event.held_rust);
    }
    assert_eq!(live, 0);
    drop(grant);
    assert_eq!(root.snapshot().rust_bytes, 0);
    Ok(())
}

#[test]
fn actual_vec_growth_allocations_and_realloc_overlap_have_prior_grant() -> Result<(), NumericReject>
{
    use crate::selected_p1::resource::owner::{OwnerLayout, SharedOwner};
    let n = 513;
    let bound = maximum_length(Layout::new::<u64>(), 0, n)?;
    let model = OwnerLayout::actual::<Vec<u64>>(0)?;
    let bytes = model.required_rust(bound.peak_bytes)?;
    let root = ProcessLedger::new(bytes, [u32::MAX; CLASS_COUNT]);
    let audit = AuditScope::start(&root);
    let grant = root.try_reserve(
        Amount {
            rust_bytes: bytes,
            c_bytes: 0,
            slots: 1,
        },
        ResourceClass::Source,
        Admission {
            epoch: 1,
            source: None,
        },
    )?;
    let owner = SharedOwner::try_init(grant, 0, bound.peak_bytes, |_| {
        let mut values = Vec::new();
        for value in 0..n {
            values.push(value as u64);
        }
        Ok(values)
    })?;
    assert_eq!(owner.value().len(), n);
    drop(owner);
    let report = audit.finish();
    assert!(!report.overflow);
    assert!(report.events().iter().any(|event| event.kind == 3));
    for event in report.events() {
        assert_eq!(event.held_rust, bytes);
        if event.kind == 3 {
            assert!(event.old_size + event.size <= bound.peak_bytes);
        }
    }
    assert_eq!(
        report.events().last().map(|event| event.size),
        Some(model.backing.size())
    );
    assert_eq!(root.snapshot().rust_bytes, 0);
    Ok(())
}
