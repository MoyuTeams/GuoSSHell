#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rshell_m0::rshell_session::ExternalSigner as _;
use rshell_m0::russh::keys::HashAlg;
use rshell_m0::russh::keys::signature::Verifier;
use rshell_m0::russh::keys::ssh_encoding::Decode as _;
use rshell_m0::russh::keys::ssh_key::{EcdsaCurve, Signature};
use secrecy::SecretString;
use tokio::sync::mpsc;

use super::virtual_card::{CARDHOLDER, CardKey, IDENT, PIN, VirtualCard};
use super::{CardContext, CardFailure, CardSigner};
use crate::external_signer::SignerRequest;

fn cards(card: &VirtualCard) -> Arc<CardContext> {
    Arc::new(CardContext::new(Arc::new(card.clone())))
}

fn pin(value: &str) -> SecretString {
    SecretString::from(value.to_owned())
}

#[test]
fn scanning_reads_the_authentication_key_and_pin_state() {
    let card = VirtualCard::new(CardKey::ed25519([7; 32]), false);
    let found = cards(&card).scan(false).expect("scan");
    assert_eq!(found.len(), 1);
    let info = &found[0];
    assert_eq!(info.ident, IDENT);
    assert_eq!(info.cardholder, CARDHOLDER);
    assert_eq!(info.algorithm, "Ed25519");
    assert_eq!(
        info.public_key.as_ref().map(|key| key.key_data().clone()),
        Some(card.public_key().key_data().clone())
    );
    assert_eq!(info.pin_tries_left, 3);
    assert!(!info.touch);
}

#[test]
fn the_card_signs_after_the_pin_and_the_signature_verifies() {
    let card = VirtualCard::new(CardKey::ed25519([7; 32]), true);
    let cards = cards(&card);
    let key = card.public_key();
    let data = b"session identifier and userauth request";
    assert_eq!(
        cards.sign(IDENT, &key, pin("000000"), data, None, false, &|_| {}),
        Err(CardFailure::PinWrong { tries_left: 2 })
    );

    let touched = AtomicBool::new(false);
    let blob = cards
        .sign(IDENT, &key, pin(PIN), data, None, false, &|touch| {
            touched.store(touch, Ordering::SeqCst);
        })
        .expect("signature");
    assert!(touched.load(Ordering::SeqCst), "the card asks for a touch");
    let signature = Signature::decode(&mut blob.as_slice()).expect("SSH signature blob");
    Verifier::verify(&key, data, &signature).expect("valid Ed25519 signature");
    assert_eq!(card.tries_left(), 3, "a correct PIN resets the counter");
}

#[test]
fn three_wrong_pins_block_the_card() {
    let card = VirtualCard::new(CardKey::ed25519([7; 32]), false);
    let cards = cards(&card);
    let key = card.public_key();
    for (attempt, expected) in [
        ("1111", CardFailure::PinWrong { tries_left: 2 }),
        ("2222", CardFailure::PinWrong { tries_left: 1 }),
        ("3333", CardFailure::PinBlocked),
        (PIN, CardFailure::PinBlocked),
    ] {
        assert_eq!(
            cards.sign(IDENT, &key, pin(attempt), b"x", None, false, &|_| {}),
            Err(expected)
        );
    }
    assert_eq!(cards.probe(IDENT, &key).expect("probe").pin_tries_left, 0);
}

#[test]
fn another_key_or_another_card_is_refused() {
    let card = VirtualCard::new(CardKey::ed25519([7; 32]), false);
    let cards = cards(&card);
    let other = VirtualCard::new(CardKey::ed25519([8; 32]), false).public_key();
    assert_eq!(
        cards.probe(IDENT, &other).err(),
        Some(CardFailure::KeyMismatch)
    );
    assert_eq!(
        cards.sign(IDENT, &other, pin(PIN), b"x", None, false, &|_| {}),
        Err(CardFailure::KeyMismatch)
    );
    assert_eq!(
        cards.probe("0006:12345678", &card.public_key()).err(),
        Some(CardFailure::NotFound)
    );
    assert_eq!(
        card.tries_left(),
        3,
        "a mismatched key never reaches VERIFY"
    );
}

#[tokio::test]
async fn the_signer_asks_again_after_a_wrong_pin_and_remembers_on_request() {
    let card = VirtualCard::new(CardKey::ed25519([7; 32]), false);
    let cards = cards(&card);
    let (requests, mut questions) = mpsc::unbounded_channel();
    let signer = CardSigner::new(
        cards.clone(),
        1,
        "probe@test:22".to_owned(),
        "laptop card".to_owned(),
        IDENT.to_owned(),
        card.public_key(),
        requests,
    );
    let answers = tokio::spawn(async move {
        let mut asked = Vec::new();
        for answer in ["000000", PIN] {
            let (question, reply) = loop {
                match questions.recv().await {
                    Some(SignerRequest::Pin { question, reply }) => break (question, reply),
                    Some(SignerRequest::Waiting(_)) => {}
                    None => return asked,
                }
            };
            asked.push((question.key_name, question.tries_left, question.retry));
            let _ = reply.send(Some((pin(answer), true)));
        }
        asked
    });
    let blob = signer.sign(b"data", None).await.expect("signed");
    assert!(!blob.is_empty());
    assert_eq!(
        answers.await.expect("answers"),
        vec![
            ("laptop card".to_owned(), Some(3), false),
            ("laptop card".to_owned(), Some(2), true),
        ]
    );

    // 记住了 PIN：再签名不问。
    let (requests, mut questions) = mpsc::unbounded_channel();
    let again = CardSigner::new(
        cards.clone(),
        2,
        String::new(),
        String::new(),
        IDENT.to_owned(),
        card.public_key(),
        requests,
    );
    again
        .sign(b"data", None)
        .await
        .expect("signed with the remembered PIN");
    while let Ok(request) = questions.try_recv() {
        assert!(
            matches!(request, SignerRequest::Waiting(_)),
            "no PIN question"
        );
    }
}

#[tokio::test]
async fn cancelling_the_pin_or_a_missing_card_is_reported_by_the_signer() {
    let card = VirtualCard::new(CardKey::ed25519([7; 32]), false);
    let (requests, mut questions) = mpsc::unbounded_channel();
    let signer = CardSigner::new(
        cards(&card),
        1,
        String::new(),
        String::new(),
        IDENT.to_owned(),
        card.public_key(),
        requests,
    );
    tokio::spawn(async move {
        while let Some(request) = questions.recv().await {
            if let SignerRequest::Pin { reply, .. } = request {
                let _ = reply.send(None);
            }
        }
    });
    assert!(signer.sign(b"data", None).await.is_err());
    assert_eq!(signer.failure(), Some(CardFailure::Cancelled));

    let (requests, _questions) = mpsc::unbounded_channel();
    let elsewhere = CardSigner::new(
        cards(&VirtualCard::new(CardKey::ed25519([9; 32]), false)),
        1,
        String::new(),
        String::new(),
        "0006:12345678".to_owned(),
        card.public_key(),
        requests,
    );
    assert!(elsewhere.sign(b"data", None).await.is_err());
    assert_eq!(elsewhere.failure(), Some(CardFailure::NotFound));
}

/// 读卡、按 `hash` 签名，并用读到的公钥验证签名；返回签名算法名。
fn sign_and_verify(card: &VirtualCard, hash: Option<HashAlg>) -> String {
    let cards = cards(card);
    let found = cards.scan(false).expect("scan");
    let key = found[0].public_key.clone().expect("supported key");
    assert_eq!(key.key_data(), card.public_key().key_data());
    let data = b"session identifier and userauth request";
    let blob = cards
        .sign(IDENT, &key, pin(PIN), data, hash, false, &|_| {})
        .expect("signature");
    let signature = Signature::decode(&mut blob.as_slice()).expect("SSH signature blob");
    Verifier::verify(&key, data, &signature).expect("valid signature");
    signature.algorithm().to_string()
}

#[test]
fn rsa_cards_sign_with_the_sha2_variant_the_server_accepts() {
    let card = VirtualCard::new(CardKey::rsa().expect("RSA test key"), false);
    let found = cards(&card).scan(false).expect("scan");
    assert_eq!(found[0].algorithm, "RSA 2048");
    assert_eq!(
        sign_and_verify(&card, Some(HashAlg::Sha256)),
        "rsa-sha2-256"
    );
    assert_eq!(
        sign_and_verify(&card, Some(HashAlg::Sha512)),
        "rsa-sha2-512"
    );
}

#[test]
fn rsa_cards_fall_back_to_ssh_rsa_for_servers_without_sha2() {
    let card = VirtualCard::new(CardKey::rsa().expect("RSA test key"), false);
    let key = card.public_key();
    let blob = cards(&card)
        .sign(IDENT, &key, pin(PIN), b"legacy", None, false, &|_| {})
        .expect("signature");
    let signature = Signature::decode(&mut blob.as_slice()).expect("SSH signature blob");
    assert_eq!(signature.algorithm().to_string(), "ssh-rsa");
    assert_eq!(signature.as_bytes().len(), 256, "a 2048-bit RSA signature");
}

#[test]
fn p256_cards_return_r_and_s_as_ssh_mpints() {
    let card = VirtualCard::new(CardKey::p256([7; 32]).expect("P-256 key"), false);
    let found = cards(&card).scan(false).expect("scan");
    assert_eq!(found[0].algorithm, "NIST P-256");
    assert_eq!(sign_and_verify(&card, None), "ecdsa-sha2-nistp256");
}

#[test]
fn ecdsa_halves_drop_padding_and_keep_positive() {
    // P-384：r 以 0x00 开头（去掉），s 最高位为 1（补一个 0）。
    let mut raw = vec![0u8; 96];
    raw[1] = 0x12;
    raw[48] = 0x80;
    let blob = super::ecdsa_signature(EcdsaCurve::NistP384, &raw).expect("blob");
    let signature = Signature::decode(&mut blob.as_slice()).expect("SSH signature blob");
    assert_eq!(signature.algorithm().to_string(), "ecdsa-sha2-nistp384");
    let inner = signature.as_bytes();
    let r_len = u32::from_be_bytes(inner[..4].try_into().expect("length")) as usize;
    assert_eq!(r_len, 47);
    let s_len =
        u32::from_be_bytes(inner[4 + r_len..8 + r_len].try_into().expect("length")) as usize;
    assert_eq!(s_len, 49);
    assert_eq!(inner[8 + r_len], 0x00);

    // 每半多一个前导 0 的卡（P-521：2 × 67 字节）也照样拆。
    let padded = vec![0x01u8; 134];
    assert!(super::ecdsa_signature(EcdsaCurve::NistP521, &padded).is_ok());
    assert!(super::ecdsa_signature(EcdsaCurve::NistP521, &[0x01; 3]).is_err());
}

#[test]
fn 并发签名只尝试一次失效的缓存pin() {
    let card = VirtualCard::new(CardKey::ed25519([7; 32]), false);
    let cards = cards(&card);
    cards.remember_pin(IDENT, pin("已失效的缓存"));
    let barrier = Arc::new(std::sync::Barrier::new(9));
    let busy = super::lock(&cards.busy);
    let workers = (0..8)
        .map(|_| {
            let cards = cards.clone();
            let key = card.public_key();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                cards.sign_with_pin(
                    IDENT,
                    &key,
                    None,
                    "并发签名".as_bytes(),
                    None,
                    false,
                    &|_| {},
                )
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    drop(busy);
    let results = workers
        .into_iter()
        .map(|worker| worker.join().expect("签名线程结束"))
        .collect::<Vec<_>>();
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(CardFailure::PinWrong { tries_left: 2 })))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Ok(None)))
            .count(),
        7
    );
    assert_eq!(
        card.tries_left(),
        2,
        "等待卡锁的签名不能再次验证已失效的缓存 PIN"
    );
    assert!(cards.remembered_pin(IDENT).is_none());
}
