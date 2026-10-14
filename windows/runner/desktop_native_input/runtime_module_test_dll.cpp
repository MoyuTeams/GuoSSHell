// loader 的真实 Win32 API 回归替身；不创建 Rinf runtime 或输入 surface。
#include "../../../native/hub/desktop_native_input.h"

#define TEST_EXPORT extern "C" __declspec(dllexport)
#define TEST_NORMAL extern "C"

// 编号与 loader 的 normal 导出表一致；每个入口可独立缺失或转发。
#ifndef TEST_MISSING_EXPORT
#ifdef TEST_MISSING_NORMAL
#define TEST_MISSING_EXPORT 11
#else
#define TEST_MISSING_EXPORT 0
#endif
#endif
#ifndef TEST_FOREIGN_EXPORT
#ifdef TEST_FOREIGN_BIND
#define TEST_FOREIGN_EXPORT 2
#else
#define TEST_FOREIGN_EXPORT 0
#endif
#endif
#ifndef TEST_BAD_LAYOUT_EXPORT
#ifdef TEST_BAD_LAYOUT
#define TEST_BAD_LAYOUT_EXPORT 2
#else
#define TEST_BAD_LAYOUT_EXPORT 0
#endif
#endif
#ifndef TEST_BAD_LAYOUT_KIND
#define TEST_BAD_LAYOUT_KIND 1
#endif
#ifndef TEST_MUTATE_OUTPUT_EXPORT
#define TEST_MUTATE_OUTPUT_EXPORT 0
#endif
#ifndef TEST_MUTATE_PAYLOAD_BUFFER
#define TEST_MUTATE_PAYLOAD_BUFFER 0
#endif
#ifndef TEST_BAD_BOUND_LAYOUT_KIND
#define TEST_BAD_BOUND_LAYOUT_KIND 0
#endif
#ifndef TEST_ONLY_NORMAL_EXPORT
#define TEST_ONLY_NORMAL_EXPORT 0
#endif

#if TEST_MISSING_EXPORT == 1 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 1)
#elif TEST_FOREIGN_EXPORT == 1
#pragma comment(linker, "/export:guossh_normal_v1_register_trusted_host=duplicate.guossh_normal_v1_register_trusted_host")
#else
#pragma comment(linker, "/export:guossh_normal_v1_register_trusted_host")
#endif
#if TEST_MISSING_EXPORT == 2 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 2)
#elif TEST_FOREIGN_EXPORT == 2
#pragma comment(linker, "/export:guossh_normal_v1_bind=duplicate.guossh_normal_v1_bind")
#else
#pragma comment(linker, "/export:guossh_normal_v1_bind")
#endif
#if TEST_MISSING_EXPORT == 3 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 3)
#elif TEST_FOREIGN_EXPORT == 3
#pragma comment(linker, "/export:guossh_normal_v1_key=duplicate.guossh_normal_v1_key")
#else
#pragma comment(linker, "/export:guossh_normal_v1_key")
#endif
#if TEST_MISSING_EXPORT == 4 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 4)
#elif TEST_FOREIGN_EXPORT == 4
#pragma comment(linker, "/export:guossh_normal_v1_composition=duplicate.guossh_normal_v1_composition")
#else
#pragma comment(linker, "/export:guossh_normal_v1_composition")
#endif
#if TEST_MISSING_EXPORT == 5 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 5)
#elif TEST_FOREIGN_EXPORT == 5
#pragma comment(linker, "/export:guossh_normal_v1_action=duplicate.guossh_normal_v1_action")
#else
#pragma comment(linker, "/export:guossh_normal_v1_action")
#endif
#if TEST_MISSING_EXPORT == 6 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 6)
#elif TEST_FOREIGN_EXPORT == 6
#pragma comment(linker, "/export:guossh_normal_v1_claim=duplicate.guossh_normal_v1_claim")
#else
#pragma comment(linker, "/export:guossh_normal_v1_claim")
#endif
#if TEST_MISSING_EXPORT == 7 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 7)
#elif TEST_FOREIGN_EXPORT == 7
#pragma comment(linker, "/export:guossh_normal_v1_action_payload=duplicate.guossh_normal_v1_action_payload")
#else
#pragma comment(linker, "/export:guossh_normal_v1_action_payload")
#endif
#if TEST_MISSING_EXPORT == 8 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 8)
#elif TEST_FOREIGN_EXPORT == 8
#pragma comment(linker, "/export:guossh_normal_v1_ack=duplicate.guossh_normal_v1_ack")
#else
#pragma comment(linker, "/export:guossh_normal_v1_ack")
#endif
#if TEST_MISSING_EXPORT == 9 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 9)
#elif TEST_FOREIGN_EXPORT == 9
#pragma comment(linker, "/export:guossh_normal_v1_projection=duplicate.guossh_normal_v1_projection")
#else
#pragma comment(linker, "/export:guossh_normal_v1_projection")
#endif
#if TEST_MISSING_EXPORT == 10 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 10)
#elif TEST_FOREIGN_EXPORT == 10
#pragma comment(linker, "/export:guossh_normal_v1_revoke=duplicate.guossh_normal_v1_revoke")
#else
#pragma comment(linker, "/export:guossh_normal_v1_revoke")
#endif
#if TEST_MISSING_EXPORT == 11 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 11)
#elif TEST_FOREIGN_EXPORT == 11
#pragma comment(linker, "/export:guossh_normal_v1_release_retired=duplicate.guossh_normal_v1_release_retired")
#else
#pragma comment(linker, "/export:guossh_normal_v1_release_retired")
#endif
#if TEST_MISSING_EXPORT == 12 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 12)
#elif TEST_FOREIGN_EXPORT == 12
#pragma comment(linker, "/export:guossh_normal_v1_reserve_payload=duplicate.guossh_normal_v1_reserve_payload")
#else
#pragma comment(linker, "/export:guossh_normal_v1_reserve_payload")
#endif
#if TEST_MISSING_EXPORT == 13 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 13)
#elif TEST_FOREIGN_EXPORT == 13
#pragma comment(linker, "/export:guossh_normal_v1_payload=duplicate.guossh_normal_v1_payload")
#else
#pragma comment(linker, "/export:guossh_normal_v1_payload")
#endif
#if TEST_MISSING_EXPORT == 14 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 14)
#elif TEST_FOREIGN_EXPORT == 14
#pragma comment(linker, "/export:guossh_normal_v1_finish=duplicate.guossh_normal_v1_finish")
#else
#pragma comment(linker, "/export:guossh_normal_v1_finish")
#endif
#if TEST_MISSING_EXPORT == 15 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 15)
#elif TEST_FOREIGN_EXPORT == 15
#pragma comment(linker, "/export:guossh_normal_v1_text_end_query=duplicate.guossh_normal_v1_text_end_query")
#else
#pragma comment(linker, "/export:guossh_normal_v1_text_end_query")
#endif
#if TEST_MISSING_EXPORT == 16 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 16)
#elif TEST_FOREIGN_EXPORT == 16
#pragma comment(linker, "/export:guossh_normal_v1_text_end_ack=duplicate.guossh_normal_v1_text_end_ack")
#else
#pragma comment(linker, "/export:guossh_normal_v1_text_end_ack")
#endif
#if TEST_MISSING_EXPORT == 17 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 17)
#elif TEST_FOREIGN_EXPORT == 17
#pragma comment(linker, "/export:guossh_normal_v1_presented_geometry=duplicate.guossh_normal_v1_presented_geometry")
#else
#pragma comment(linker, "/export:guossh_normal_v1_presented_geometry")
#endif
#if TEST_MISSING_EXPORT == 18 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 18)
#elif TEST_FOREIGN_EXPORT == 18
#pragma comment(linker, "/export:guossh_normal_v1_invalidate_geometry=duplicate.guossh_normal_v1_invalidate_geometry")
#else
#pragma comment(linker, "/export:guossh_normal_v1_invalidate_geometry")
#endif
#if TEST_MISSING_EXPORT == 19 || (TEST_ONLY_NORMAL_EXPORT != 0 && TEST_ONLY_NORMAL_EXPORT != 19)
#elif TEST_FOREIGN_EXPORT == 19
#pragma comment(linker, "/export:guossh_normal_v1_pointer=duplicate.guossh_normal_v1_pointer")
#else
#pragma comment(linker, "/export:guossh_normal_v1_pointer")
#endif

#if TEST_ONLY_NORMAL_EXPORT == 0
TEST_EXPORT void rinf_prepare_isolate_extern() {}
TEST_EXPORT void rinf_start_rust_logic_extern() {}
TEST_EXPORT void rinf_stop_rust_logic_extern() {}
TEST_EXPORT void rinf_send_dart_signal_desktop_start() {}
#ifndef TEST_MISSING_RINF
TEST_EXPORT void rinf_send_dart_signal_desktop_ordered() {}
#endif
#endif

namespace {
template <typename T>
uint32_t HeaderStatus(const T* value, int export_id) {
  // 分别模拟不拒绝版本、短布局及长布局的错误产物。
  if (export_id == TEST_BAD_LAYOUT_EXPORT &&
      ((TEST_BAD_LAYOUT_KIND == 1 && value->header.version != GUOSSH_NORMAL_ABI_VERSION) ||
       (TEST_BAD_LAYOUT_KIND == 2 && value->header.size < sizeof(T)) ||
       (TEST_BAD_LAYOUT_KIND == 3 && value->header.size > sizeof(T)))) {
    return GUOSSH_NORMAL_OK;
  }
  if (value->header.version != GUOSSH_NORMAL_ABI_VERSION) {
    return GUOSSH_NORMAL_UNSUPPORTED_VERSION;
  }
  if (value->header.size != sizeof(T)) return GUOSSH_NORMAL_INVALID_ARGUMENT;
  return GUOSSH_NORMAL_OK;
}

uint32_t ContextStatus(const GuosshNormalContext* context, int export_id) {
  const auto status = HeaderStatus(context, export_id);
  if (export_id == TEST_BAD_LAYOUT_EXPORT && status == GUOSSH_NORMAL_OK &&
      (context->header.version != GUOSSH_NORMAL_ABI_VERSION ||
       context->header.size != sizeof(*context))) return GUOSSH_NORMAL_OK;
  return status == GUOSSH_NORMAL_OK ? GUOSSH_NORMAL_UNKNOWN_HANDLE : status;
}
}  // namespace

TEST_NORMAL uint32_t guossh_normal_v1_register_trusted_host(
    const GuosshNormalRegistration* registration, GuosshNormalBound* bound) {
  if constexpr (TEST_MUTATE_OUTPUT_EXPORT == 1) bound->handle = 0;
  const auto status = HeaderStatus(registration, 1);
  if (status != GUOSSH_NORMAL_OK) return status;
  if constexpr (TEST_BAD_LAYOUT_EXPORT == 1) {
    return status;
  } else {
    const auto output_status = HeaderStatus(bound, 1);
    return output_status == GUOSSH_NORMAL_OK ? GUOSSH_NORMAL_INVALID_ARGUMENT : output_status;
  }
}
TEST_NORMAL uint32_t guossh_normal_v1_bind(
    const GuosshNormalContext* context, GuosshNormalBound* bound) {
  if constexpr (TEST_MUTATE_OUTPUT_EXPORT == 2) bound->handle = 0;
  const auto status = HeaderStatus(context, 2);
  if (status != GUOSSH_NORMAL_OK) return status;
  if constexpr (TEST_BAD_LAYOUT_EXPORT == 2) {
    return status;
  } else {
    if constexpr (TEST_BAD_BOUND_LAYOUT_KIND == 1) {
      if (bound->header.version != GUOSSH_NORMAL_ABI_VERSION) return GUOSSH_NORMAL_STALE_SOURCE;
    } else if constexpr (TEST_BAD_BOUND_LAYOUT_KIND == 2) {
      if (bound->header.size < sizeof(*bound)) return GUOSSH_NORMAL_STALE_SOURCE;
    } else if constexpr (TEST_BAD_BOUND_LAYOUT_KIND == 3) {
      if (bound->header.size > sizeof(*bound)) return GUOSSH_NORMAL_STALE_SOURCE;
    }
    const auto output_status = HeaderStatus(bound, 2);
    return output_status == GUOSSH_NORMAL_OK ? GUOSSH_NORMAL_STALE_SOURCE : output_status;
  }
}
TEST_NORMAL uint32_t guossh_normal_v1_key(
    const GuosshNormalContext* context, const GuosshNormalKey*, GuosshNormalAdmission* admission) {
  if constexpr (TEST_MUTATE_OUTPUT_EXPORT == 3) admission->end_fence = 0;
  return ContextStatus(context, 3);
}
TEST_NORMAL uint32_t guossh_normal_v1_composition(
    const GuosshNormalContext* context, const GuosshNormalComposition*) {
  return ContextStatus(context, 4);
}
TEST_NORMAL uint32_t guossh_normal_v1_action(
    const GuosshNormalContext* context, const GuosshNormalActionRequest*, uint64_t* effect) {
  if constexpr (TEST_MUTATE_OUTPUT_EXPORT == 5) *effect = 0;
  return ContextStatus(context, 5);
}
TEST_NORMAL uint32_t guossh_normal_v1_claim(
    const GuosshNormalContext* context, const GuosshNormalClaimRequest*, GuosshNormalClaimInfo* info) {
  if constexpr (TEST_MUTATE_OUTPUT_EXPORT == 6) info->claim = 0;
  return ContextStatus(context, 6);
}
TEST_NORMAL uint32_t guossh_normal_v1_action_payload(
    const GuosshNormalContext* context, const GuosshNormalPayloadCopy* copy, uint64_t* count) {
  if constexpr (TEST_MUTATE_OUTPUT_EXPORT == 7) {
    if constexpr (TEST_MUTATE_PAYLOAD_BUFFER) {
      if (copy->capacity) copy->buffer[0] = 0;
    } else {
      *count = 0;
    }
  }
  return ContextStatus(context, 7);
}
TEST_NORMAL uint32_t guossh_normal_v1_ack(
    const GuosshNormalContext* context, const GuosshNormalAck*) {
  return ContextStatus(context, 8);
}
TEST_NORMAL uint32_t guossh_normal_v1_projection(
    const GuosshNormalContext* context, GuosshNormalProjection* projection) {
  if constexpr (TEST_MUTATE_OUTPUT_EXPORT == 9) projection->row_revision = 0;
  return ContextStatus(context, 9);
}
TEST_NORMAL uint32_t guossh_normal_v1_revoke(const GuosshNormalContext* context) {
  return ContextStatus(context, 10);
}
TEST_NORMAL uint32_t guossh_normal_v1_release_retired(const GuosshNormalContext* context) {
  return ContextStatus(context, 11);
}
TEST_NORMAL uint32_t guossh_normal_v1_reserve_payload(
    const GuosshNormalContext* context, const GuosshNormalTextReservation*) {
  return ContextStatus(context, 12);
}
TEST_NORMAL uint32_t guossh_normal_v1_payload(
    const GuosshNormalContext* context, const GuosshNormalTextChunk*) {
  return ContextStatus(context, 13);
}
TEST_NORMAL uint32_t guossh_normal_v1_finish(
    const GuosshNormalContext* context, const GuosshNormalTextFinish*) {
  return ContextStatus(context, 14);
}
TEST_NORMAL uint32_t guossh_normal_v1_text_end_query(
    const GuosshNormalContext* context, GuosshNormalTextEnd* end) {
  if constexpr (TEST_MUTATE_OUTPUT_EXPORT == 15) end->end_fence = 0;
  return ContextStatus(context, 15);
}
TEST_NORMAL uint32_t guossh_normal_v1_text_end_ack(
    const GuosshNormalContext* context, const GuosshNormalTextEnd*) {
  return ContextStatus(context, 16);
}
TEST_NORMAL uint32_t guossh_normal_v1_presented_geometry(
    const GuosshNormalContext* context, const GuosshNormalPresentedGeometry*) {
  return ContextStatus(context, 17);
}
TEST_NORMAL uint32_t guossh_normal_v1_invalidate_geometry(
    const GuosshNormalContext* context, uint64_t) {
  return ContextStatus(context, 18);
}
TEST_NORMAL uint32_t guossh_normal_v1_pointer(
    const GuosshNormalContext* context, const GuosshNormalPointer*, GuosshNormalAdmission* admission) {
  if constexpr (TEST_MUTATE_OUTPUT_EXPORT == 19) admission->end_fence = 0;
  return ContextStatus(context, 19);
}
