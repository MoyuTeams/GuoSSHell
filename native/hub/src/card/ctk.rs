//! CryptoTokenKit 上的读卡器（iOS / macOS）。iOS 没有 PC/SC；macOS 上要有
//! `com.apple.security.smartcard` entitlement，否则 `defaultManager` 为 nil（看到的就是没有卡）。
//!
//! CryptoTokenKit 的调用都以回调返回；openpgp-card 的后端接口是同步的，卡操作本来就在
//! 阻塞线程上跑，这里等回调并带超时。

use std::sync::mpsc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use card_backend::{CardBackend, CardCaps, CardTransaction, PinType, SmartcardError};
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObjectProtocol};
use objc2::sel;
use objc2_crypto_token_kit::{
    TKSmartCard, TKSmartCardSlotManager, TKSmartCardSlotNFCSession, TKSmartCardSlotState,
};
use objc2_foundation::{NSData, NSError, NSString};
use rinf::debug_print;

use super::{CardFailure, CardReader, NfcSession};

const SESSION_TIMEOUT: Duration = Duration::from_secs(10);
/// 签名可能要等用户按卡上的按键。
const TRANSMIT_TIMEOUT: Duration = Duration::from_secs(60);
/// 等用户把卡靠近。
const NFC_TIMEOUT: Duration = Duration::from_secs(60);

pub struct CtkReader;

fn manager() -> Option<Retained<TKSmartCardSlotManager>> {
    // SAFETY: 类方法，没有参数；iOS 上总是可用，macOS 上没有 entitlement 时返回 nil。
    unsafe { TKSmartCardSlotManager::defaultManager() }
}

impl CardReader for CtkReader {
    fn cards(&self) -> Vec<Box<dyn CardBackend + Send + Sync>> {
        let Some(manager) = manager() else {
            return Vec::new();
        };
        let mut cards: Vec<Box<dyn CardBackend + Send + Sync>> = Vec::new();
        // SAFETY: 读取属性与按名取卡槽，参数都是有效对象。
        unsafe {
            for name in manager.slotNames().iter() {
                let Some(slot) = manager.slotNamed(&name) else {
                    continue;
                };
                if slot.state() != TKSmartCardSlotState::ValidCard {
                    continue;
                }
                if let Some(card) = slot.makeSmartCard() {
                    cards.push(Box::new(CtkCard(card)));
                }
            }
        }
        cards
    }

    fn nfc_supported(&self) -> bool {
        let Some(manager) = manager() else {
            return false;
        };
        // NFC 卡槽是 iOS 26 才有的；旧系统上没有这个方法，先问一句免得崩溃。
        // SAFETY: 确认过对象实现了这个无参方法。
        manager.respondsToSelector(sel!(isNFCSupported)) && unsafe { manager.isNFCSupported() }
    }

    fn begin_nfc(&self, message: &str) -> Result<Box<dyn NfcSession>, CardFailure> {
        let manager = manager().ok_or(CardFailure::NotFound)?;
        if !self.nfc_supported() {
            return Err(CardFailure::NotFound);
        }
        let (sender, receiver) = mpsc::channel();
        let completion = RcBlock::new(
            move |session: *mut TKSmartCardSlotNFCSession, error: *mut NSError| {
                // SAFETY: 回调给的是有效对象或 nil；retain 之后由我们持有。
                let result = match unsafe { Retained::retain(session) } {
                    Some(session) => Ok(session),
                    None => Err(describe(error)),
                };
                let _ = sender.send(result);
            },
        );
        let message = NSString::from_str(message);
        // SAFETY: 参数有效；回调只捕获了 Send 的通道。
        unsafe { manager.createNFCSlotWithMessage_completion(Some(&message), &completion) };
        let session = receiver
            .recv_timeout(NFC_TIMEOUT)
            .map_err(|_| CardFailure::NotFound)?
            .map_err(|error| {
                debug_print!("[card] NFC slot: {error}");
                CardFailure::NotFound
            })?;
        let session = NfcGuard(session);

        // 等卡靠近：这个卡槽上出现一张应答正常的卡。取消或超时都按「没有卡」处理。
        // SAFETY: 读取属性与按名取卡槽，参数都是有效对象。
        let name = unsafe { session.0.slotName() }.ok_or(CardFailure::NotFound)?;
        let deadline = Instant::now() + NFC_TIMEOUT;
        loop {
            let ready = unsafe { manager.slotNamed(&name) }
                .is_some_and(|slot| unsafe { slot.state() } == TKSmartCardSlotState::ValidCard);
            if ready {
                return Ok(Box::new(session));
            }
            if Instant::now() > deadline {
                return Err(CardFailure::NotFound);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// NFC 读卡会话；drop 时结束，系统界面收起。
struct NfcGuard(Retained<TKSmartCardSlotNFCSession>);

impl NfcSession for NfcGuard {}

impl Drop for NfcGuard {
    fn drop(&mut self) {
        // SAFETY: 会话对象有效；重复结束是无害的。
        unsafe { self.0.endSession() };
    }
}

/// 一张卡。CryptoTokenKit 的对象可以在任意线程上使用（回调在它自己的队列上），
/// 只是 objc2 默认不标 Send / Sync。
struct CtkCard(Retained<TKSmartCard>);

// SAFETY: 见上；TKSmartCard 的会话与收发都是线程安全的异步接口。
unsafe impl Send for CtkCard {}
// SAFETY: 同上。
unsafe impl Sync for CtkCard {}

impl CardBackend for CtkCard {
    fn limit_card_caps(&self, card_caps: CardCaps) -> CardCaps {
        card_caps
    }

    fn transaction(
        &mut self,
        reselect_application: Option<&[u8]>,
    ) -> Result<Box<dyn CardTransaction + Send + Sync + '_>, SmartcardError> {
        let (sender, receiver) = mpsc::channel();
        let reply = RcBlock::new(move |ok: Bool, error: *mut NSError| {
            let _ = sender.send(if ok.as_bool() {
                Ok(())
            } else {
                Err(describe(error))
            });
        });
        // SAFETY: 卡对象有效；回调只捕获了 Send 的通道。
        unsafe { self.0.beginSessionWithReply(&reply) };
        receiver
            .recv_timeout(SESSION_TIMEOUT)
            .map_err(|_| SmartcardError::Error("card session timed out".into()))?
            .map_err(SmartcardError::SmartCardConnectionError)?;

        let mut transaction = CtkTransaction { card: self };
        // 两次会话之间别的程序（系统的智能卡驱动等）可能选了别的应用。
        if let Some(application) = reselect_application {
            transaction.select(application)?;
        }
        Ok(Box::new(transaction))
    }
}

struct CtkTransaction<'a> {
    card: &'a CtkCard,
}

impl Drop for CtkTransaction<'_> {
    fn drop(&mut self) {
        // SAFETY: 会话由 transaction() 打开。
        unsafe { self.card.0.endSession() };
    }
}

impl CardTransaction for CtkTransaction<'_> {
    fn transmit(&mut self, cmd: &[u8], _buf_size: usize) -> Result<Vec<u8>, SmartcardError> {
        let request = NSData::with_bytes(cmd);
        let (sender, receiver) = mpsc::channel();
        let reply = RcBlock::new(move |response: *mut NSData, error: *mut NSError| {
            // SAFETY: 回调给的是有效对象或 nil，只在回调内读取。
            let result = match unsafe { response.as_ref() } {
                Some(response) => Ok(response.to_vec()),
                None => Err(describe(error)),
            };
            let _ = sender.send(result);
        });
        // SAFETY: 会话已打开；参数有效；回调只捕获了 Send 的通道。
        unsafe { self.card.0.transmitRequest_reply(&request, &reply) };
        receiver
            .recv_timeout(TRANSMIT_TIMEOUT)
            .map_err(|_| SmartcardError::Error("card did not answer".into()))?
            .map_err(SmartcardError::Error)
    }

    fn feature_pinpad_verify(&self) -> bool {
        false
    }

    fn feature_pinpad_modify(&self) -> bool {
        false
    }

    fn pinpad_verify(
        &mut self,
        _pin: PinType,
        _card_caps: &Option<CardCaps>,
    ) -> Result<Vec<u8>, SmartcardError> {
        Err(SmartcardError::Error("no pinpad".into()))
    }

    fn pinpad_modify(
        &mut self,
        _pin: PinType,
        _card_caps: &Option<CardCaps>,
    ) -> Result<Vec<u8>, SmartcardError> {
        Err(SmartcardError::Error("no pinpad".into()))
    }

    fn was_reset(&self) -> bool {
        false
    }
}

fn describe(error: *mut NSError) -> String {
    // SAFETY: 回调给的是有效对象或 nil，只在这里读取。
    unsafe { error.as_ref() }
        .map(|error| error.localizedDescription().to_string())
        .unwrap_or_else(|| "unknown CryptoTokenKit error".to_owned())
}
