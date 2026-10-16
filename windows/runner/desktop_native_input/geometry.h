#pragma once
#include "codec.h"
#include "../native_input_surface/contracts.h"

namespace guosshell::desktop_native_input {
struct Geometry {
  GuosshNormalGeometry native{};
  double x{}, y{}, width{}, height{}, local_width{}, local_height{};
  double cell_width{}, cell_height{}, scroll_offset{}, dpr{};
  uint64_t viewport_revision{};
  bool valid{};
  static bool Parse(const Map&, Geometry*);
  bool Matches(const Map&, const GuosshNormalProjection&) const;
  bool Local(double global_x, double global_y, double* local_x, double* local_y) const noexcept;
  bool Representable(double left, double top, double right, double bottom) const noexcept;
  POINT Screen(HWND view, double local_x, double local_y) const noexcept;
  RECT Rect(HWND view, double left, double top, double right, double bottom) const noexcept;
  RECT Clip(HWND view) const noexcept;
  bool Position(HWND window, HWND view) const noexcept;
  guossh::native_input::SourceGeometry TextGeometry(HWND view, const GuosshNormalProjection&, uint64_t source) const noexcept;
};
}  // namespace guosshell::desktop_native_input
