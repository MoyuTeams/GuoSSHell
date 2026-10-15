use std::alloc::Layout;

use super::{
    Failure, SnapshotBody,
    error_bound::{self, ErrorUpper},
    raw::{FieldValue, RawFields},
};
use crate::selected_p1::{
    decode_shape::{self, ContainerKind, ScratchFrame, Shape},
    model::{
        capacity::{cautious, maximum_length, tree_clone_backing, tree_insert_peak},
        certificate::{GNU_1981, TargetLayouts},
        layout::{NumericReject, PublicLayouts, add, array},
    },
    resource::owner::OwnerLayout,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AllocationClasses {
    pub owned_text: usize,
    pub value_containers: usize,
    pub binding_vectors: usize,
    pub tag_tree: usize,
    pub content: usize,
    pub parser_scratch: usize,
    pub errors: usize,
    pub resolve: usize,
    pub snapshot_owner: usize,
}

impl AllocationClasses {
    pub fn total(self) -> Result<usize, NumericReject> {
        [
            self.owned_text,
            self.value_containers,
            self.binding_vectors,
            self.tag_tree,
            self.content,
            self.parser_scratch,
            self.errors,
            self.resolve,
            self.snapshot_owner,
        ]
        .into_iter()
        .try_fold(0, add)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JsonPlan {
    pub shape: Option<Shape>,
    pub value_containers: usize,
    pub binding_vectors: usize,
    pub content_containers: usize,
    pub clone_containers: usize,
    pub error: ErrorUpper,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodePlan {
    pub classes: AllocationClasses,
    /// override/app/settings 固定三项；无连接时 override 为零。
    pub json: [JsonPlan; 3],
    pub rust_bytes: usize,
}

pub(super) fn validate_target(target: &TargetLayouts) -> Result<(), Failure> {
    if target.build != GNU_1981 || target.public != PublicLayouts::actual() {
        return Err(Failure::Resource(NumericReject::UnsupportedEnvelope));
    }
    target.private_bounds()?;
    Ok(())
}

pub fn scratch_requirement(
    raw: &RawFields<'_>,
) -> Result<decode_shape::ScratchRequirement, Failure> {
    let mut largest =
        decode_shape::needed_scratch(raw.text(22).as_bytes()).map_err(Failure::shape)?;
    for tag in [16, 19] {
        if tag == 16 && !raw.saved() {
            continue;
        }
        let requirement =
            decode_shape::needed_scratch(raw.text(tag).as_bytes()).map_err(Failure::shape)?;
        if requirement.slots > largest.slots {
            largest = requirement;
        }
    }
    Ok(largest)
}

fn json_plan(
    text: &str,
    scratch: &mut [ScratchFrame],
    target: &TargetLayouts,
) -> Result<JsonPlan, Failure> {
    let private = target.private_bounds()?;
    let mut plan = JsonPlan::default();
    let mut model_error = None;
    let shape = decode_shape::scan_with(text.as_bytes(), scratch, |container| {
        let result = (|| -> Result<(), NumericReject> {
            let n = container.started_members;
            match container.kind {
                ContainerKind::Array => {
                    plan.value_containers = add(
                        plan.value_containers,
                        maximum_length(target.public.value, 0, n)?.peak_bytes,
                    )?;
                    plan.binding_vectors = add(
                        plan.binding_vectors,
                        maximum_length(target.public.key_binding, 0, n)?
                            .peak_bytes
                            .max(
                                maximum_length(
                                    target.public.key_binding,
                                    cautious(target.public.key_binding, n),
                                    n,
                                )?
                                .peak_bytes,
                            ),
                    )?;
                    plan.content_containers = add(
                        plan.content_containers,
                        maximum_length(private.content, 0, n)?.peak_bytes,
                    )?;
                    plan.clone_containers =
                        add(plan.clone_containers, array(target.public.value, n)?.size())?;
                }
                ContainerKind::Object => {
                    plan.value_containers = add(
                        plan.value_containers,
                        tree_insert_peak(private.string_value_nodes, n)?,
                    )?;
                    plan.content_containers = add(
                        plan.content_containers,
                        add(
                            maximum_length(private.content_pair, 0, n)?.peak_bytes,
                            maximum_length(private.flatten_entry, 0, n)?.peak_bytes,
                        )?,
                    )?;
                    plan.clone_containers = add(
                        plan.clone_containers,
                        tree_clone_backing(private.string_value_nodes, n)?,
                    )?;
                }
            }
            Ok(())
        })();
        result.map_err(|error| {
            model_error = Some(error);
            decode_shape::NumericReject::ArithmeticOverflow
        })
    })
    .map_err(|error| model_error.map_or_else(|| Failure::shape(error), Failure::Resource))?;
    plan.error = error_bound::bound(
        shape.costs,
        shape.position_value_upper().map_err(Failure::shape)?,
        target,
    )?;
    plan.shape = Some(shape);
    Ok(plan)
}

/// 此函数仅借用 raw 与已准入 scratch；首次 resolve 界不读取任何 typed 图。
pub fn bound_selected(
    raw: &RawFields<'_>,
    scratch: &mut [ScratchFrame],
    target: &TargetLayouts,
) -> Result<DecodePlan, Failure> {
    validate_target(target)?;
    let mut classes = AllocationClasses::default();
    if raw.saved() {
        for field in raw.fields() {
            if matches!(field.tag, 3 | 4 | 6 | 9 | 10 | 12 | 13 | 21 | 23)
                && let FieldValue::Text(text) = field.value
            {
                classes.owned_text = add(classes.owned_text, text.len())?;
            }
        }
        classes.tag_tree = tree_insert_peak(
            target.private_bounds()?.string_set_nodes,
            raw.tags().count(),
        )?;
    } else {
        classes.owned_text = raw.text(21).len();
        let quick = raw.selection.quick.ok_or(Failure::Protocol)?;
        classes.owned_text = add(
            classes.owned_text,
            add(add(quick.host.len(), quick.username.len())?, 1)?,
        )?;
        classes.owned_text = add(
            classes.owned_text,
            add(
                add(quick.host.len(), quick.username.len())?,
                quick.command.len(),
            )?,
        )?;
    }
    let mut json = [JsonPlan::default(); 3];
    for (index, tag) in [16, 19, 22].into_iter().enumerate() {
        if tag == 16 && !raw.saved() {
            continue;
        }
        json[index] = json_plan(raw.text(tag), scratch, target)?;
        let plan = json[index];
        let shape = plan.shape.ok_or(Failure::Protocol)?;
        classes.owned_text = add(classes.owned_text, shape.costs.decoded_string_bytes_upper)?;
        classes.value_containers = add(classes.value_containers, plan.value_containers)?;
        classes.binding_vectors = add(classes.binding_vectors, plan.binding_vectors)?;
        classes.content = add(
            classes.content,
            add(
                plan.content_containers,
                shape.costs.decoded_string_bytes_upper,
            )?,
        )?;
        // 三个 from_str 顺序调用、各 scratch/error 在返回前析构；typed 图仍由前项全部占账。
        classes.parser_scratch = classes.parser_scratch.max(
            maximum_length(
                Layout::new::<u8>(),
                0,
                shape.costs.parser_scratch_required_upper(),
            )?
            .peak_bytes,
        );
        classes.errors = classes.errors.max(plan.error.total);
    }
    let settings = json[2].shape.ok_or(Failure::Protocol)?.costs;
    let overrides = json[0].shape.map(|shape| shape.costs).unwrap_or_default();
    let app = json[1].shape.ok_or(Failure::Protocol)?.costs;
    let k = add(
        settings.arrays.started_members,
        overrides.arrays.started_members,
    )?;
    let a = app.arrays.started_members;
    let text = settings
        .max_decoded_string_bytes_upper
        .max(overrides.max_decoded_string_bytes_upper);
    let mut resolve = "xterm-256color".len() + "CaskaydiaCove NF Mono".len() + "rsHell".len();
    for default in ["xterm-256color", "CaskaydiaCove NF Mono", "rsHell"] {
        resolve = add(resolve, text.max(default.len()))?;
    }
    resolve = add(
        resolve,
        maximum_length(target.public.key_binding, k, add(k, a)?)?.peak_bytes,
    )?;
    resolve = add(
        resolve,
        add(
            add(
                settings.decoded_string_bytes_upper,
                overrides.decoded_string_bytes_upper,
            )?,
            app.decoded_string_bytes_upper,
        )?,
    )?;
    resolve = add(
        resolve,
        add(
            json[2].clone_containers,
            settings.decoded_string_bytes_upper,
        )?,
    )?;
    classes.resolve = resolve;
    classes.snapshot_owner = OwnerLayout::actual::<SnapshotBody>(0)?.backing.size();
    let rust_bytes = classes.total()?;
    Ok(DecodePlan {
        classes,
        json,
        rust_bytes,
    })
}
