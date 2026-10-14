#pragma once

#include <flutter/encodable_value.h>
#include <charconv>
#include <cmath>
#include <string>
#include "runtime_module.h"

namespace guosshell::desktop_native_input {
using Value = flutter::EncodableValue;
using Map = flutter::EncodableMap;
using List = flutter::EncodableList;

template<class T> T Abi() noexcept {
  T value{};
  value.header = {GUOSSH_NORMAL_ABI_VERSION, static_cast<uint32_t>(sizeof(T))};
  return value;
}
inline const Value* Find(const Map& map, const char* name) {
  const auto it = map.find(Value(name));
  return it == map.end() ? nullptr : &it->second;
}
inline const Map* Object(const Map& map, const char* name) {
  const auto* value = Find(map, name);
  return value ? std::get_if<Map>(value) : nullptr;
}
inline const List* Array(const Map& map, const char* name) {
  const auto* value = Find(map, name);
  return value ? std::get_if<List>(value) : nullptr;
}
inline const std::string* String(const Map& map, const char* name) {
  const auto* value = Find(map, name);
  return value ? std::get_if<std::string>(value) : nullptr;
}
inline bool U64(const Map& map, const char* name, uint64_t* out) {
  const auto* text = String(map, name);
  if (!text || text->empty() || text->size() > 20 ||
      (text->size() > 1 && (*text)[0] == '0')) return false;
  const auto result = std::from_chars(text->data(), text->data() + text->size(), *out);
  return result.ec == std::errc{} && result.ptr == text->data() + text->size();
}
inline bool Integer(const Map& map, const char* name, int64_t* out) {
  const auto* value = Find(map, name);
  if (!value) return false;
  if (const auto* n = std::get_if<int32_t>(value)) { *out = *n; return true; }
  if (const auto* n = std::get_if<int64_t>(value)) { *out = *n; return true; }
  return false;
}
inline bool Number(const Value& value, double* out) {
  if (const auto* number = std::get_if<double>(&value)) *out = *number;
  else if (const auto* int32_value = std::get_if<int32_t>(&value)) *out = *int32_value;
  else if (const auto* int64_value = std::get_if<int64_t>(&value)) *out = static_cast<double>(*int64_value);
  else return false;
  return std::isfinite(*out);
}
inline bool Number(const Map& map, const char* name, double* out) {
  const auto* value = Find(map, name);
  return value && Number(*value, out);
}
inline Map State(const char* state) {
  return {{Value("version"), Value(int32_t{1})}, {Value("state"), Value(state)}};
}
inline void Put(Map& map, const char* name, uint64_t value) { map[Value(name)] = Value(std::to_string(value)); }
inline bool Version(const Map& map) {
  int64_t version{};
  return Integer(map, "version", &version) && version == 1;
}
}  // namespace guosshell::desktop_native_input
