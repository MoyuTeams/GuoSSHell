//! 槽元数据的新 owned 副本先收费；无旧 Arc 接管或事后补签。

use crate::fonts::ImportedFont;
use crate::selected_p1::model::layout::{NumericReject, add};
use crate::selected_p1::resource::ledger::{Admission, Amount, ProcessLedger, ResourceClass};
use crate::selected_p1::resource::owner::{OwnerLayout, SharedOwner};

#[repr(C)]
struct SlotBody {
    version: u64,
    font: Option<ImportedFont>,
}

pub struct BudgetedLegacySlot<'root>(SharedOwner<'root, SlotBody>);

impl<'root> BudgetedLegacySlot<'root> {
    /// 父从已借用偏好取得输入；该旧偏好数据根仍须由父持有并计费。
    /// 本构造仅证明新槽副本，不能给旧 preferences/get 或 FontResource backing 补签。
    pub fn try_initial(
        ledger: &'root ProcessLedger,
        admission: Admission,
        version: u64,
        font: Option<&ImportedFont>,
    ) -> Result<Self, NumericReject> {
        let backing = font.map_or(Ok(0), |font| add(font.hash.len(), font.label.len()))?;
        let layout = OwnerLayout::actual::<SlotBody>(0)?;
        let grant = ledger.try_reserve(
            Amount {
                rust_bytes: layout.required_rust(backing)?,
                c_bytes: 0,
                slots: 1,
            },
            ResourceClass::Source,
            admission,
        )?;
        SharedOwner::try_init(grant, 0, backing, |_| {
            let font = font
                .map(|font| {
                    Ok(ImportedFont {
                        hash: exact_copy(&font.hash)?,
                        label: exact_copy(&font.label)?,
                    })
                })
                .transpose()?;
            Ok(SlotBody { version, font })
        })
        .map(Self)
    }

    /// 父在同一偏好锁内调用并发布返回值；版本 checked_add，不能用配置 revision 替代。
    /// 创建失败时旧槽及其版本仍原样存活，替换不影响已经捕获的 Source。
    pub fn try_replacement(
        &self,
        ledger: &'root ProcessLedger,
        admission: Admission,
        font: Option<&ImportedFont>,
    ) -> Result<Self, NumericReject> {
        let version = self
            .version()
            .checked_add(1)
            .ok_or(NumericReject::Overflow)?;
        Self::try_initial(ledger, admission, version, font)
    }

    pub fn version(&self) -> u64 {
        self.0.value().version
    }

    pub fn font(&self) -> Option<&ImportedFont> {
        self.0.value().font.as_ref()
    }

    pub fn admitted(&self) -> Amount {
        self.0.admitted()
    }
}

fn exact_copy(input: &str) -> Result<String, NumericReject> {
    let mut output = String::new();
    output
        .try_reserve_exact(input.len())
        .map_err(|_| NumericReject::AllocationFailure)?;
    output.push_str(input);
    Ok(output)
}
