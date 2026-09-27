# 把 mocknvml 自带的 L40S 配置改成 M6 用的：两块 GPU，温度 / 功耗 / 利用率随时间变化
# （nvtop 的曲线才会动）。在 `system:` 与 `device_defaults:` 两个顶层键下各插一段。
{ print }
/^system:/ {
  print "  num_devices: 2"
}
/^device_defaults:/ {
  print "  dynamic_metrics:"
  print "    seed: 6"
  print "    temperature: { base_c: 48, variance_c: 3, ramp_c: 25, ramp_period_sec: 20 }"
  print "    power: { base_mw: 220000, variance_mw: 80000 }"
  print "    utilization: { pattern: burst, gpu_min: 5, gpu_max: 100, memory_min: 10, memory_max: 90, burst_period_sec: 5 }"
}
