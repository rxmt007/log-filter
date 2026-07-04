import sys

# 用法: python scripts/gen_biglog.py <输出路径> <目标MB>
path = sys.argv[1]
target_bytes = int(sys.argv[2]) * 1024 * 1024

line = "04-20 12:06:{:02d}.{:03d}   146   179 D BatteryService: update start seq={}\n"
written = 0
i = 0
with open(path, "w", encoding="utf-8") as f:
    while written < target_bytes:
        s = line.format(i % 60, i % 1000, i)
        f.write(s)
        written += len(s)
        i += 1
print(f"wrote {written} bytes, {i} lines -> {path}")
