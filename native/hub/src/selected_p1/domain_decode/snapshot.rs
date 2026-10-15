use std::alloc::Layout;
use std::collections::BTreeMap;

use rshell_m0::rshell_core::{
    AppSettings, ConnectionProfile, KeyBinding, ResolvedTerminalProfile, TerminalOverrides,
    TerminalProfile, TerminalSettingsV1,
};
use serde_json::Value;

use super::raw::{ProfileSource, SelectedReferences};
use crate::selected_p1::model::{
    capacity::tree_clone_backing,
    certificate::TargetLayouts,
    layout::{NumericReject, add, array},
};

/// 完整 Rust 快照；私有字段不因 Dart 有限投影而删减。Source/font 由独立 owner 组合。
#[derive(Clone, Debug, PartialEq)]
#[repr(C)]
pub struct SnapshotBody {
    pub target: ConnectionProfile,
    pub saved: bool,
    pub profile: TerminalProfile,
    pub app: AppSettings,
    pub resolved: ResolvedTerminalProfile,
    pub revision: u64,
    pub source: ProfileSource,
    pub selected_references: SelectedReferences,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CloneShape {
    /// 新 clone 的 out-of-line 请求；原图必须继续由原 owner 占账。
    pub clone_bytes: usize,
    /// 原图真实 capacities 加已证节点数上界，供临时量析构后收紧。
    pub retained_bytes: usize,
}

impl CloneShape {
    fn include(&mut self, clone: usize, retained: usize) -> Result<(), NumericReject> {
        self.clone_bytes = add(self.clone_bytes, clone)?;
        self.retained_bytes = add(self.retained_bytes, retained)?;
        Ok(())
    }
    fn string(&mut self, value: &String) -> Result<(), NumericReject> {
        self.include(value.len(), value.capacity())
    }
    fn bindings(
        &mut self,
        value: &Vec<KeyBinding>,
        target: &TargetLayouts,
    ) -> Result<(), NumericReject> {
        self.include(
            array(target.public.key_binding, value.len())?.size(),
            array(target.public.key_binding, value.capacity())?.size(),
        )?;
        for binding in value {
            self.string(&binding.action)?;
        }
        Ok(())
    }
    fn map(
        &mut self,
        value: &BTreeMap<String, Value>,
        target: &TargetLayouts,
    ) -> Result<(), NumericReject> {
        let bytes = tree_clone_backing(target.private_bounds()?.string_value_nodes, value.len())?;
        self.include(bytes, bytes)?;
        for (key, value) in value {
            self.string(key)?;
            self.value(value, target)?;
        }
        Ok(())
    }
    fn value(&mut self, value: &Value, target: &TargetLayouts) -> Result<(), NumericReject> {
        match value {
            Value::String(text) => self.string(text)?,
            Value::Array(values) => {
                self.include(
                    array(target.public.value, values.len())?.size(),
                    array(target.public.value, values.capacity())?.size(),
                )?;
                for value in values {
                    self.value(value, target)?;
                }
            }
            Value::Object(map) => {
                let bytes =
                    tree_clone_backing(target.private_bounds()?.string_value_nodes, map.len())?;
                self.include(bytes, bytes)?;
                for (key, value) in map {
                    self.string(key)?;
                    self.value(value, target)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn settings(
        &mut self,
        value: &TerminalSettingsV1,
        target: &TargetLayouts,
    ) -> Result<(), NumericReject> {
        self.string(&value.terminal_type)?;
        self.string(&value.font_family)?;
        self.string(&value.answerback)?;
        self.bindings(&value.key_bindings, target)?;
        self.map(&value.extensions, target)
    }
    fn overrides(
        &mut self,
        value: &TerminalOverrides,
        target: &TargetLayouts,
    ) -> Result<(), NumericReject> {
        for text in [&value.terminal_type, &value.font_family, &value.answerback]
            .into_iter()
            .flatten()
        {
            self.string(text)?;
        }
        if let Some(bindings) = &value.key_bindings {
            self.bindings(bindings, target)?;
        }
        Ok(())
    }
}

impl SnapshotBody {
    pub fn actual_layout() -> Layout {
        Layout::new::<Self>()
    }

    pub fn clone_shape(&self, target: &TargetLayouts) -> Result<CloneShape, NumericReject> {
        let mut shape = CloneShape::default();
        let connection = &self.target;
        for text in [
            &connection.name,
            &connection.host,
            &connection.username,
            &connection.note,
        ] {
            shape.string(text)?;
        }
        if let Some(value) = &connection.credential_ref {
            shape.string(&value.0)?;
        }
        if let Some(path) = &connection.identity_file {
            shape.include(path.as_os_str().len(), path.capacity())?;
        }
        if let Some(text) = &connection.remote_command {
            shape.string(text)?;
        }
        let tree = tree_clone_backing(
            target.private_bounds()?.string_set_nodes,
            connection.tags.len(),
        )?;
        shape.include(tree, tree)?;
        for tag in &connection.tags {
            shape.string(tag)?;
        }
        shape.overrides(&connection.terminal_overrides, target)?;
        shape.string(&self.profile.name)?;
        shape.settings(&self.profile.settings, target)?;
        shape.bindings(&self.app.key_bindings, target)?;
        for text in [
            &self.resolved.terminal_type,
            &self.resolved.font_family,
            &self.resolved.answerback,
        ] {
            shape.string(text)?;
        }
        shape.bindings(&self.resolved.key_bindings, target)?;
        shape.map(&self.resolved.extensions, target)?;
        Ok(shape)
    }
}
