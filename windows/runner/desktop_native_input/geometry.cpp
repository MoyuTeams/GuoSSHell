#include "geometry.h"
#include <algorithm>
#include <array>

namespace guosshell::desktop_native_input {
bool Geometry::Parse(const Map& map, Geometry* out) {
  Geometry g{};
  const auto* clip = Object(map, "clip");
  const auto* matrix = Array(map, "transform");
  double left{}, top{}, right{}, bottom{};
  if (!clip || !matrix || matrix->size() != 16 ||
      !Number(map, "x", &g.x) || !Number(map, "y", &g.y) ||
      !Number(map, "width", &g.width) || !Number(map, "height", &g.height) ||
      !Number(map, "cellWidth", &g.cell_width) || !Number(map, "cellHeight", &g.cell_height) ||
      !Number(map, "scrollOffset", &g.scroll_offset) || !Number(map, "devicePixelRatio", &g.dpr) ||
      !Number(*clip, "left", &left) || !Number(*clip, "top", &top) ||
      !Number(*clip, "right", &right) || !Number(*clip, "bottom", &bottom) ||
      !U64(map, "fontRevision", &g.native.font_revision) || !U64(map, "layoutRevision", &g.native.layout_revision) ||
      !U64(map, "transformRevision", &g.native.transform_revision) || !U64(map, "viewportRevision", &g.viewport_revision)) return false;
  std::array<double, 16> m{};
  for (size_t i = 0; i < m.size(); ++i) if (!Number((*matrix)[i], &m[i])) return false;
  // Matrix4 是 column-major；只接受真正二维 affine，不丢弃 perspective/Z 分量。
  for (const size_t i : {2u, 3u, 6u, 7u, 8u, 9u, 11u, 14u}) if (m[i] != 0) return false;
  const double affine_determinant = m[0] * m[5] - m[1] * m[4];
  if (m[10] != 1 || m[15] != 1 || !std::isfinite(affine_determinant) || std::abs(affine_determinant) < 1e-12) return false;
  g.native.transform[0] = m[0]; g.native.transform[1] = m[1];
  g.native.transform[2] = m[4]; g.native.transform[3] = m[5];
  g.native.transform[4] = m[12]; g.native.transform[5] = m[13];
  if (g.width <= 0 || g.height <= 0 || g.cell_width <= 0 || g.cell_height <= 0 ||
      g.scroll_offset < 0 || g.dpr <= 0 || g.dpr > 16 || right <= left || bottom <= top ||
      left < g.x - .01 || top < g.y - .01 || right > g.x + g.width + .01 || bottom > g.y + g.height + .01) return false;
  for (double n : {g.x, g.y, g.width, g.height, left, right, top, bottom})
    if (std::abs(n) > 10000000 || std::abs(n) * g.dpr > 10000000) return false;
  // 从已转换 bounds 恢复原矩形；退化的轴向包围盒不足以证明输入区域。
  const double determinant = std::abs(m[0] * m[5]) - std::abs(m[1] * m[4]);
  if (std::abs(determinant) < 1e-12) return false;
  g.local_width = (g.width * std::abs(m[5]) - g.height * std::abs(m[4])) / determinant;
  g.local_height = (g.height * std::abs(m[0]) - g.width * std::abs(m[1])) / determinant;
  if (!std::isfinite(g.local_width) || !std::isfinite(g.local_height) || g.local_width <= 0 || g.local_height <= 0 ||
      g.local_width / g.cell_width > UINT32_MAX || g.local_height / g.cell_height > UINT32_MAX) return false;
  const double bound_left = m[12] + std::min(0.0, m[0] * g.local_width) + std::min(0.0, m[4] * g.local_height);
  const double bound_top = m[13] + std::min(0.0, m[1] * g.local_width) + std::min(0.0, m[5] * g.local_height);
  if (std::abs(bound_left - g.x) > .01 || std::abs(bound_top - g.y) > .01 ||
      std::abs(std::abs(m[0]) * g.local_width + std::abs(m[4]) * g.local_height - g.width) > .01 ||
      std::abs(std::abs(m[1]) * g.local_width + std::abs(m[5]) * g.local_height - g.height) > .01) return false;
  g.native.clip_x = left; g.native.clip_y = top;
  g.native.clip_width = right - left; g.native.clip_height = bottom - top;
  *out = g;
  return true;
}
bool Geometry::Matches(const Map& map, const GuosshNormalProjection& p) const {
  int64_t frame{}, viewport{}; uint64_t row{};
  if (!Integer(map, "frameRevision", &frame) || frame != p.frame_revision ||
      !U64(map, "rowRevision", &row) || row != p.row_revision ||
      !Integer(map, "viewportTop", &viewport) || viewport != p.viewport_top) return false;
  const auto* value = Find(map, "caret");
  if (!p.caret_present) return value && value->IsNull();
  const auto* caret = Object(map, "caret");
  if (!caret) return false;
  double x0{}, y0{}, w{}, h{}; int64_t dpi{};
  return Number(*caret, "x", &x0) && x0 == p.caret_x && Number(*caret, "y", &y0) && y0 == p.caret_y &&
      Number(*caret, "width", &w) && w == p.caret_width && Number(*caret, "height", &h) && h == p.caret_height &&
      Integer(*caret, "dpi", &dpi) && dpi == p.dpi;
}
bool Geometry::Local(double gx, double gy, double* lx, double* ly) const noexcept {
  if (gx < native.clip_x || gy < native.clip_y || gx >= native.clip_x + native.clip_width ||
      gy >= native.clip_y + native.clip_height) return false;
  const double* m = native.transform;
  const double dx = gx - m[4], dy = gy - m[5], determinant = m[0] * m[3] - m[1] * m[2];
  *lx = (m[3] * dx - m[2] * dy) / determinant;
  *ly = (m[0] * dy - m[1] * dx) / determinant;
  return *lx >= 0 && *ly >= 0 && *lx < local_width && *ly < local_height;
}
bool Geometry::Representable(double l, double t, double r, double b) const noexcept {
  const double* m = native.transform;
  for (double px : {l, r}) for (double py : {t, b}) {
    const double x0 = (m[0] * px + m[2] * py + m[4]) * dpr;
    const double y0 = (m[1] * px + m[3] * py + m[5]) * dpr;
    if (!std::isfinite(x0) || !std::isfinite(y0) || std::abs(x0) > 10000000 || std::abs(y0) > 10000000) return false;
  }
  return true;
}
POINT Geometry::Screen(HWND view, double lx, double ly) const noexcept {
  const double* m = native.transform;
  POINT p{static_cast<LONG>(std::lround((m[0] * lx + m[2] * ly + m[4]) * dpr)),
          static_cast<LONG>(std::lround((m[1] * lx + m[3] * ly + m[5]) * dpr))};
  ClientToScreen(view, &p);
  return p;
}
RECT Geometry::Rect(HWND view, double l, double t, double r, double b) const noexcept {
  const std::array<POINT, 4> points{Screen(view, l, t), Screen(view, r, t), Screen(view, r, b), Screen(view, l, b)};
  RECT rect{points[0].x, points[0].y, points[0].x, points[0].y};
  for (auto p : points) {
    rect.left = std::min(rect.left, p.x); rect.right = std::max(rect.right, p.x);
    rect.top = std::min(rect.top, p.y); rect.bottom = std::max(rect.bottom, p.y);
  }
  return rect;
}
RECT Geometry::Clip(HWND view) const noexcept {
  POINT origin{}; ClientToScreen(view, &origin);
  return {origin.x + static_cast<LONG>(std::ceil(native.clip_x * dpr)),
          origin.y + static_cast<LONG>(std::ceil(native.clip_y * dpr)),
          origin.x + static_cast<LONG>(std::floor((native.clip_x + native.clip_width) * dpr)),
          origin.y + static_cast<LONG>(std::floor((native.clip_y + native.clip_height) * dpr))};
}
bool Geometry::Position(HWND window, HWND view) const noexcept {
  const RECT screen = Clip(view);
  POINT origin{screen.left, screen.top}; ScreenToClient(view, &origin);
  if (screen.right <= screen.left || screen.bottom <= screen.top ||
      !SetWindowPos(window, HWND_TOP, origin.x, origin.y, screen.right - screen.left, screen.bottom - screen.top,
                    SWP_NOACTIVATE)) return false;
  std::array<POINT, 4> polygon{Screen(view, 0, 0), Screen(view, local_width, 0),
      Screen(view, local_width, local_height), Screen(view, 0, local_height)};
  for (auto& p : polygon) { p.x -= screen.left; p.y -= screen.top; }
  HRGN region = CreatePolygonRgn(polygon.data(), static_cast<int>(polygon.size()), WINDING);
  HRGN clip = CreateRectRgn(0, 0, screen.right - screen.left, screen.bottom - screen.top);
  if (!region || !clip) { if (region) DeleteObject(region); if (clip) DeleteObject(clip); return false; }
  const int combined = CombineRgn(region, region, clip, RGN_AND);
  DeleteObject(clip);
  if (combined == ERROR || !SetWindowRgn(window, region, TRUE)) { DeleteObject(region); return false; }
  return true;
}
guossh::native_input::SourceGeometry Geometry::TextGeometry(HWND view, const GuosshNormalProjection& p, uint64_t source) const noexcept {
  guossh::native_input::SourceGeometry g{};
  g.revision = {source, p.frame_revision, viewport_revision, native.font_revision, native.transform_revision};
  g.screen_clip = Clip(view);
  if (!valid || !p.caret_present || !Representable(p.caret_x, p.caret_y, p.caret_x + p.caret_width, p.caret_y + p.caret_height)) return g;
  g.cursor_screen = Rect(view, p.caret_x, p.caret_y, p.caret_x + p.caret_width, p.caret_y + p.caret_height);
  g.acknowledged = valid && p.caret_present != 0;
  return g;
}
}  // namespace guosshell::desktop_native_input
