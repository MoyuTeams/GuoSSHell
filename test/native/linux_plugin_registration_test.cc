// 原生注册回归：未显式启用 Windows 专用功能时，不改变 Linux 窗口和关闭处理。
#include <flutter_linux/flutter_linux.h>
#include <gtk/gtk.h>

#include <flutter_acrylic/flutter_acrylic_plugin.h>
#include <window_manager/window_manager_plugin.h>

static gboolean on_close(GtkWidget*, GdkEvent*, gpointer) {
  return FALSE;
}

int main(int argc, char** argv) {
  gtk_init(&argc, &argv);
  g_autoptr(FlDartProject) project = fl_dart_project_new();
  GtkWidget* window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
  FlView* view = fl_view_new(project);
  gtk_container_add(GTK_CONTAINER(window), GTK_WIDGET(view));
  const auto close_handler = g_signal_connect(
      window, "delete-event", G_CALLBACK(on_close), view);
  const auto paintable = gtk_widget_get_app_paintable(window);
  GdkVisual* visual = gtk_widget_get_visual(window);

  g_autoptr(FlPluginRegistrar) manager =
      fl_plugin_registry_get_registrar_for_plugin(
          FL_PLUGIN_REGISTRY(view), "WindowManagerPlugin");
  window_manager_plugin_register_with_registrar(manager);
  g_assert_true(g_signal_handler_is_connected(window, close_handler));
  g_assert_false(gtk_widget_get_visible(window));

  g_autoptr(FlPluginRegistrar) acrylic =
      fl_plugin_registry_get_registrar_for_plugin(
          FL_PLUGIN_REGISTRY(view), "FlutterAcrylicPlugin");
  flutter_acrylic_plugin_register_with_registrar(acrylic);
  g_assert_cmpint(gtk_widget_get_app_paintable(window), ==, paintable);
  g_assert_true(gtk_widget_get_visual(window) == visual);
  g_assert_false(gtk_widget_get_visible(window));
  g_assert_false(gtk_widget_get_visible(GTK_WIDGET(view)));
  g_assert_true(g_signal_handler_is_connected(window, close_handler));
  gtk_widget_destroy(window);
  g_print("Linux 插件注册保持窗口外观、可见性和关闭处理不变。\n");
  return 0;
}
