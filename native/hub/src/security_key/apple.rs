//! AuthenticationServices 上的安全密钥（iOS / macOS）：系统界面引导用户插上、靠近或触摸
//! 安全密钥。请求要在主线程上发起，结果经委托回来；调用方在阻塞线程上等。
//!
//! RP ID 取 Info.plist 的 `GUOSHSecurityKeyRelyingParty`：必须是 App 经 Associated Domains
//! （`webcredentials:`）关联的域名，系统才接受；留空或没有这一项时不提供安全密钥。

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
};
use objc2_authentication_services::{
    ASAuthorization, ASAuthorizationAllSupportedPublicKeyCredentialDescriptorTransports,
    ASAuthorizationController, ASAuthorizationControllerDelegate,
    ASAuthorizationControllerPresentationContextProviding,
    ASAuthorizationPublicKeyCredentialAssertion,
    ASAuthorizationPublicKeyCredentialAssertionRequest,
    ASAuthorizationPublicKeyCredentialParameters, ASAuthorizationPublicKeyCredentialRegistration,
    ASAuthorizationPublicKeyCredentialRegistrationRequest,
    ASAuthorizationPublicKeyCredentialResidentKeyPreferenceDiscouraged,
    ASAuthorizationPublicKeyCredentialUserVerificationPreferenceDiscouraged,
    ASAuthorizationRequest, ASAuthorizationSecurityKeyPublicKeyCredentialAssertion,
    ASAuthorizationSecurityKeyPublicKeyCredentialDescriptor,
    ASAuthorizationSecurityKeyPublicKeyCredentialProvider,
    ASAuthorizationSecurityKeyPublicKeyCredentialRegistration, ASCOSEAlgorithmIdentifierES256,
    ASPublicKeyCredential,
};
use objc2_foundation::{NSArray, NSBundle, NSData, NSError, NSString};

use super::{Assertion, Authenticator, Registration, SkFailure};

/// 等用户操作的上限（系统界面自己也会超时关掉）。
const USER_TIMEOUT: Duration = Duration::from_secs(180);
/// `ASAuthorizationError.canceled`。
const ERROR_CANCELED: isize = 1001;
/// Info.plist 里安全密钥 RP ID 的键。
const RELYING_PARTY_KEY: &str = "GUOSHSecurityKeyRelyingParty";

pub struct AppleSecurityKeys;

impl Authenticator for AppleSecurityKeys {
    fn relying_party(&self) -> Option<String> {
        let value = NSBundle::mainBundle()
            .objectForInfoDictionaryKey(&NSString::from_str(RELYING_PARTY_KEY))?;
        let value = value.downcast::<NSString>().ok()?.to_string();
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    }

    fn register(&self, rp_id: &str, user_name: &str) -> Result<Registration, SkFailure> {
        match perform(Request::Register {
            rp_id: rp_id.to_owned(),
            user_name: user_name.to_owned(),
        }) {
            Outcome::Registration(registration) => Ok(registration),
            Outcome::Assertion(_) => Err(SkFailure::Failed("unexpected assertion".into())),
            Outcome::Failed(failure) => Err(failure),
        }
    }

    fn assert(
        &self,
        rp_id: &str,
        credential_id: &[u8],
        challenge: &[u8],
    ) -> Result<Assertion, SkFailure> {
        match perform(Request::Assert {
            rp_id: rp_id.to_owned(),
            credential_id: credential_id.to_vec(),
            challenge: challenge.to_vec(),
        }) {
            Outcome::Assertion(assertion) => Ok(assertion),
            Outcome::Registration(_) => Err(SkFailure::Failed("unexpected registration".into())),
            Outcome::Failed(failure) => Err(failure),
        }
    }
}

enum Request {
    Register {
        rp_id: String,
        user_name: String,
    },
    Assert {
        rp_id: String,
        credential_id: Vec<u8>,
        challenge: Vec<u8>,
    },
}

enum Outcome {
    Registration(Registration),
    Assertion(Assertion),
    Failed(SkFailure),
}

/// 在主线程上发起请求，在当前（阻塞）线程上等结果。等不到就收起系统界面。
fn perform(request: Request) -> Outcome {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (reply, outcome) = mpsc::channel();
    DispatchQueue::main().exec_async(move || {
        if let Some(mtm) = MainThreadMarker::new() {
            start(mtm, id, request, reply);
        }
    });
    outcome.recv_timeout(USER_TIMEOUT).unwrap_or_else(|_| {
        DispatchQueue::main().exec_async(move || cancel(id));
        Outcome::Failed(SkFailure::Failed("security key timed out".into()))
    })
}

/// 收起还开着的系统界面（控制器随后以取消回调委托）。
fn cancel(id: u64) {
    IN_FLIGHT.with(|slot| {
        for delegate in slot
            .borrow()
            .iter()
            .filter(|delegate| delegate.ivars().id == id)
        {
            if let Some(controller) = delegate.ivars().controller.borrow().as_ref() {
                // SAFETY: 主线程上取消一个进行中的请求。
                unsafe { controller.cancel() };
            }
        }
    });
}

thread_local! {
    /// 进行中的请求的委托（控制器对委托只是弱引用）。只在主线程上用。
    static IN_FLIGHT: RefCell<Vec<Retained<Delegate>>> = const { RefCell::new(Vec::new()) };
}

fn start(mtm: MainThreadMarker, id: u64, request: Request, reply: Sender<Outcome>) {
    let authorization_request = match build_request(request) {
        Ok(request) => request,
        Err(failure) => {
            let _ = reply.send(Outcome::Failed(failure));
            return;
        }
    };
    let delegate = Delegate::new(mtm, id, reply);
    // SAFETY: 主线程；参数都是有效对象。
    unsafe {
        let controller = ASAuthorizationController::initWithAuthorizationRequests(
            ASAuthorizationController::alloc(),
            &NSArray::from_retained_slice(&[authorization_request]),
        );
        controller.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        controller.setPresentationContextProvider(Some(ProtocolObject::from_ref(&*delegate)));
        *delegate.ivars().controller.borrow_mut() = Some(controller.clone());
        IN_FLIGHT.with(|slot| slot.borrow_mut().push(delegate.clone()));
        controller.performRequests();
    }
}

fn build_request(request: Request) -> Result<Retained<ASAuthorizationRequest>, SkFailure> {
    // SAFETY: 构造与设置请求对象，参数都是有效对象。
    unsafe {
        match request {
            Request::Register { rp_id, user_name } => {
                let provider = provider(&rp_id);
                // 挑战与用户 id 只是注册的形式要求：服务器不参与注册，不校验它们。
                let challenge = NSData::with_bytes(uuid::Uuid::new_v4().as_bytes());
                let user_id = NSData::with_bytes(uuid::Uuid::new_v4().as_bytes());
                let name = NSString::from_str(&user_name);
                let request = provider
                    .createCredentialRegistrationRequestWithChallenge_displayName_name_userID(
                        &challenge, &name, &name, &user_id,
                    );
                let es256 = ASAuthorizationPublicKeyCredentialParameters::initWithAlgorithm(
                    ASAuthorizationPublicKeyCredentialParameters::alloc(),
                    ASCOSEAlgorithmIdentifierES256,
                );
                request.setCredentialParameters(&NSArray::from_retained_slice(&[es256]));
                if let Some(discouraged) =
                    ASAuthorizationPublicKeyCredentialResidentKeyPreferenceDiscouraged
                {
                    request.setResidentKeyPreference(discouraged);
                }
                if let Some(discouraged) =
                    ASAuthorizationPublicKeyCredentialUserVerificationPreferenceDiscouraged
                {
                    request.setUserVerificationPreference(discouraged);
                }
                Ok(Retained::into_super(request))
            }
            Request::Assert {
                rp_id,
                credential_id,
                challenge,
            } => {
                let provider = provider(&rp_id);
                let request = provider
                    .createCredentialAssertionRequestWithChallenge(&NSData::with_bytes(&challenge));
                let descriptor =
                    ASAuthorizationSecurityKeyPublicKeyCredentialDescriptor::initWithCredentialID_transports(
                        ASAuthorizationSecurityKeyPublicKeyCredentialDescriptor::alloc(),
                        &NSData::with_bytes(&credential_id),
                        &ASAuthorizationAllSupportedPublicKeyCredentialDescriptorTransports(),
                    );
                request.setAllowedCredentials(&NSArray::from_retained_slice(&[descriptor]));
                if let Some(discouraged) =
                    ASAuthorizationPublicKeyCredentialUserVerificationPreferenceDiscouraged
                {
                    request.setUserVerificationPreference(discouraged);
                }
                Ok(Retained::into_super(request))
            }
        }
    }
}

fn provider(rp_id: &str) -> Retained<ASAuthorizationSecurityKeyPublicKeyCredentialProvider> {
    // SAFETY: 初始化方法，参数是有效字符串。
    unsafe {
        ASAuthorizationSecurityKeyPublicKeyCredentialProvider::initWithRelyingPartyIdentifier(
            ASAuthorizationSecurityKeyPublicKeyCredentialProvider::alloc(),
            &NSString::from_str(rp_id),
        )
    }
}

struct Ivars {
    id: u64,
    reply: RefCell<Option<Sender<Outcome>>>,
    /// 请求期间留着控制器（超时要用它收起界面）。
    controller: RefCell<Option<Retained<ASAuthorizationController>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "GuoSSHellSecurityKeyDelegate"]
    #[ivars = Ivars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl ASAuthorizationControllerDelegate for Delegate {
        #[unsafe(method(authorizationController:didCompleteWithAuthorization:))]
        fn did_complete(
            &self,
            _controller: &ASAuthorizationController,
            authorization: &ASAuthorization,
        ) {
            self.finish(outcome_of(authorization));
        }

        #[unsafe(method(authorizationController:didCompleteWithError:))]
        fn did_fail(&self, _controller: &ASAuthorizationController, error: &NSError) {
            let failure = if error.code() == ERROR_CANCELED {
                SkFailure::Cancelled
            } else {
                SkFailure::Failed(format!(
                    "{} [{} {}]",
                    error.localizedDescription(),
                    error.domain(),
                    error.code()
                ))
            };
            self.finish(Outcome::Failed(failure));
        }
    }

    unsafe impl ASAuthorizationControllerPresentationContextProviding for Delegate {
        #[unsafe(method(presentationAnchorForAuthorizationController:))]
        fn anchor(&self, _controller: &ASAuthorizationController) -> *mut AnyObject {
            Retained::autorelease_return(key_window(self.mtm()))
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker, id: u64, reply: Sender<Outcome>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            id,
            reply: RefCell::new(Some(reply)),
            controller: RefCell::new(None),
        });
        // SAFETY: NSObject 的 init。
        unsafe { msg_send![super(this), init] }
    }

    /// 交出结果，离开这次回调之后再放掉委托（控制器随它一起放掉）。
    fn finish(&self, outcome: Outcome) {
        if let Some(reply) = self.ivars().reply.borrow_mut().take() {
            let _ = reply.send(outcome);
        }
        DispatchQueue::main().exec_async(|| {
            IN_FLIGHT.with(|slot| {
                slot.borrow_mut()
                    .retain(|delegate| delegate.ivars().reply.borrow().is_some());
            });
        });
    }
}

fn outcome_of(authorization: &ASAuthorization) -> Outcome {
    // SAFETY: 读取结果对象的属性。
    unsafe {
        let credential = authorization.credential();
        let credential: &AnyObject = credential.as_ref();
        if let Some(registration) =
            credential.downcast_ref::<ASAuthorizationSecurityKeyPublicKeyCredentialRegistration>()
        {
            let Some(attestation_object) = registration.rawAttestationObject() else {
                return Outcome::Failed(SkFailure::Invalid("no attestation object".into()));
            };
            return Outcome::Registration(Registration {
                credential_id: registration.credentialID().to_vec(),
                attestation_object: attestation_object.to_vec(),
            });
        }
        if let Some(assertion) =
            credential.downcast_ref::<ASAuthorizationSecurityKeyPublicKeyCredentialAssertion>()
        {
            return Outcome::Assertion(Assertion {
                authenticator_data: assertion.rawAuthenticatorData().to_vec(),
                client_data_json: assertion.rawClientDataJSON().to_vec(),
                signature: assertion.signature().to_vec(),
            });
        }
    }
    Outcome::Failed(SkFailure::Invalid("unexpected credential type".into()))
}

/// 系统界面挂在当前的主窗口上。
#[cfg(target_os = "ios")]
fn key_window(mtm: MainThreadMarker) -> Retained<AnyObject> {
    use objc2_ui_kit::{UIApplication, UISceneActivationState, UIWindow, UIWindowScene};
    let window = UIApplication::sharedApplication(mtm)
        .connectedScenes()
        .iter()
        .filter(|scene| scene.activationState() == UISceneActivationState::ForegroundActive)
        .find_map(|scene| scene.downcast::<UIWindowScene>().ok()?.keyWindow())
        .unwrap_or_else(|| UIWindow::new(mtm));
    Retained::into_super(Retained::into_super(Retained::into_super(
        Retained::into_super(window),
    )))
}

#[cfg(target_os = "macos")]
fn key_window(mtm: MainThreadMarker) -> Retained<AnyObject> {
    use objc2_app_kit::{NSApplication, NSWindow};
    let window = NSApplication::sharedApplication(mtm)
        .keyWindow()
        // SAFETY: 主线程上新建一个窗口。
        .unwrap_or_else(|| unsafe { NSWindow::new(mtm) });
    Retained::into_super(Retained::into_super(Retained::into_super(window)))
}
