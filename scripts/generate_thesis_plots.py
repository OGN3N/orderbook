#!/usr/bin/env python3
"""Generate dependency-free SVG figures from an order-book result CSV.

The benchmark CSV contains aggregate percentiles rather than raw observations,
so this script produces percentile comparisons, not histograms or empirical CDFs.
"""

from __future__ import annotations

import argparse
import csv
import math
from pathlib import Path
from xml.sax.saxutils import escape


IMPLEMENTATIONS = ("fixed_tick", "soa", "hybrid", "tree")
IMPLEMENTATION_LABELS = {
    "fixed_tick": "Fixed-tick AoS",
    "soa": "Fixed-tick SoA",
    "hybrid": "Hybrid",
    "tree": "B-tree AoS",
}
OPERATIONS = ("add_order", "cancel_order", "market_order")
OPERATION_LABELS = {
    "add_order": "Limit-order insertion",
    "cancel_order": "Cancellation",
    "market_order": "Market-order execution",
}
COLORS = {
    "fixed_tick": "#2563EB",
    "soa": "#DC2626",
    "hybrid": "#059669",
    "tree": "#7C3AED",
}
FONT = "Arial, Helvetica, sans-serif"
SCENARIO_TITLES = {
    "uniform": "Uniform workload",
    "clustered": "Clustered workload",
    "zipfian": "Zipfian workload",
    "bursty": "Bursty workload",
    "high_cancel": "High-cancellation workload",
    "sweep": "Market-sweep workload",
    "buildup": "Order-book build-up workload",
    "alignment": "Alignment and padding experiment",
    "prefetch": "Software-prefetch experiment",
}
SWEEP_CASES = (("small", 5), ("medium", 20), ("large", 50), ("cross_zone", 150))
SWEEP_DIRECTIONS = ("buy", "sell")
BUILDUP_WINDOWS = (
    ("add_depth_0_499", 0, 499),
    ("add_depth_2500_2999", 2_500, 2_999),
    ("add_depth_5000_5499", 5_000, 5_499),
    ("add_depth_7500_7999", 7_500, 7_999),
    ("add_depth_10000_10499", 10_000, 10_499),
)
ALIGNMENT_LAYOUTS = ("default_24b", "packed_17b", "aligned_64b")
ALIGNMENT_LABELS = {
    "default_24b": "Default (24 B)",
    "packed_17b": "Packed (17 B)",
    "aligned_64b": "Aligned (64 B)",
}
ALIGNMENT_COLORS = {
    "default_24b": "#2563EB",
    "packed_17b": "#F97316",
    "aligned_64b": "#7C3AED",
}
ALIGNMENT_OPERATIONS = (
    ("sequential_scan_10000", "Sequential scan", "10,000 records"),
    ("random_read_batch_64", "Random access", "64 reads"),
    ("insert_batch_10000", "Vec construction", "10,000 pushes"),
)
PREFETCH_OPERATIONS = (
    (
        "sequential_scan_10000_levels",
        "Sequential level scan",
        "10,000 header checks",
        (
            ("no_prefetch", "None"),
            ("prefetch_header_plus_4", "Header +4"),
            ("prefetch_header_plus_16", "Header +16"),
        ),
    ),
    (
        "random_access_10000_reads",
        "Known random access",
        "10,000 header reads",
        (
            ("no_prefetch", "None"),
            ("prefetch_header_plus_1", "Header +1"),
            ("prefetch_header_plus_4", "Header +4"),
        ),
    ),
    (
        "pointer_chase_10000_levels",
        "Sparse pointer chase",
        "10,000 headers + 1,465 orders",
        (
            ("no_prefetch", "None"),
            ("prefetch_heap_plus_2", "Heap +2"),
            ("prefetch_heap_plus_8", "Heap +8"),
        ),
    ),
    (
        "market_sweep_20_levels_60_orders",
        "Simulated market sweep",
        "scan to 5,000; consume 60 orders",
        (
            ("no_prefetch", "None"),
            ("prefetch_heap_window_4", "Heap ≤4"),
        ),
    ),
)
PREFETCH_COLORS = ("#2563EB", "#F97316", "#7C3AED")


class Svg:
    def __init__(self, width: int, height: int) -> None:
        self.width = width
        self.height = height
        self.items: list[str] = []

    def rect(
        self,
        x: float,
        y: float,
        width: float,
        height: float,
        *,
        fill: str = "none",
        stroke: str = "none",
        stroke_width: float = 1,
        opacity: float = 1,
        rx: float = 0,
    ) -> None:
        self.items.append(
            f'<rect x="{x:.2f}" y="{y:.2f}" width="{width:.2f}" '
            f'height="{height:.2f}" fill="{fill}" stroke="{stroke}" '
            f'stroke-width="{stroke_width}" opacity="{opacity}" rx="{rx}"/>'
        )

    def line(
        self,
        x1: float,
        y1: float,
        x2: float,
        y2: float,
        *,
        stroke: str = "#111827",
        stroke_width: float = 1,
        dash: str | None = None,
    ) -> None:
        dash_attr = f' stroke-dasharray="{dash}"' if dash else ""
        self.items.append(
            f'<line x1="{x1:.2f}" y1="{y1:.2f}" x2="{x2:.2f}" '
            f'y2="{y2:.2f}" stroke="{stroke}" stroke-width="{stroke_width}"{dash_attr}/>'
        )

    def text(
        self,
        x: float,
        y: float,
        value: str,
        *,
        size: int = 16,
        anchor: str = "middle",
        weight: str = "normal",
        fill: str = "#111827",
        rotate: float | None = None,
    ) -> None:
        transform = f' transform="rotate({rotate:.1f} {x:.2f} {y:.2f})"' if rotate else ""
        self.items.append(
            f'<text x="{x:.2f}" y="{y:.2f}" font-family="{FONT}" '
            f'font-size="{size}" font-weight="{weight}" fill="{fill}" '
            f'text-anchor="{anchor}"{transform}>{escape(value)}</text>'
        )

    def polyline(
        self,
        points: list[tuple[float, float]],
        *,
        stroke: str,
        stroke_width: float = 3,
    ) -> None:
        encoded = " ".join(f"{x:.2f},{y:.2f}" for x, y in points)
        self.items.append(
            f'<polyline points="{encoded}" fill="none" stroke="{stroke}" '
            f'stroke-width="{stroke_width}" stroke-linejoin="round" '
            f'stroke-linecap="round"/>'
        )

    def circle(self, x: float, y: float, radius: float, *, fill: str) -> None:
        self.items.append(
            f'<circle cx="{x:.2f}" cy="{y:.2f}" r="{radius:.2f}" fill="{fill}"/>'
        )

    def write(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        body = "\n  ".join(self.items)
        path.write_text(
            f'<svg xmlns="http://www.w3.org/2000/svg" width="{self.width}" '
            f'height="{self.height}" viewBox="0 0 {self.width} {self.height}">\n'
            f'  <rect width="100%" height="100%" fill="white"/>\n  {body}\n</svg>\n',
            encoding="utf-8",
        )


def load_rows(path: Path) -> dict[tuple[str, str], dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        rows = list(csv.DictReader(handle))

    indexed = {(row["operation"], row["implementation"]): row for row in rows}
    missing = [
        (operation, implementation)
        for operation in OPERATIONS
        for implementation in IMPLEMENTATIONS
        if (operation, implementation) not in indexed
    ]
    if missing:
        formatted = ", ".join(f"{op}/{impl}" for op, impl in missing)
        raise ValueError(f"CSV is missing expected rows: {formatted}")
    return indexed


def load_sweep_rows(path: Path) -> dict[tuple[str, str], dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        rows = list(csv.DictReader(handle))

    indexed = {(row["operation"], row["implementation"]): row for row in rows}
    expected_operations = [
        f"{case}_{direction}_sweep"
        for case, _ in SWEEP_CASES
        for direction in SWEEP_DIRECTIONS
    ]
    missing = [
        (operation, implementation)
        for operation in expected_operations
        for implementation in IMPLEMENTATIONS
        if (operation, implementation) not in indexed
    ]
    if missing:
        formatted = ", ".join(f"{op}/{impl}" for op, impl in missing)
        raise ValueError(f"Sweep CSV is missing expected rows: {formatted}")
    return indexed


def load_buildup_rows(path: Path) -> dict[tuple[str, str], dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        rows = list(csv.DictReader(handle))

    indexed = {(row["operation"], row["implementation"]): row for row in rows}
    missing = [
        (operation, implementation)
        for operation, _, _ in BUILDUP_WINDOWS
        for implementation in IMPLEMENTATIONS
        if (operation, implementation) not in indexed
    ]
    if missing:
        formatted = ", ".join(f"{op}/{impl}" for op, impl in missing)
        raise ValueError(f"Build-up CSV is missing expected rows: {formatted}")
    return indexed


def load_alignment_rows(path: Path) -> dict[tuple[str, str], dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        rows = list(csv.DictReader(handle))

    indexed = {(row["operation"], row["implementation"]): row for row in rows}
    missing = [
        (operation, layout)
        for operation, _, _ in ALIGNMENT_OPERATIONS
        for layout in ALIGNMENT_LAYOUTS
        if (operation, layout) not in indexed
    ]
    if missing:
        formatted = ", ".join(f"{op}/{layout}" for op, layout in missing)
        raise ValueError(f"Alignment CSV is missing expected rows: {formatted}")
    return indexed


def load_prefetch_rows(path: Path) -> dict[tuple[str, str], dict[str, str]]:
    with path.open(newline="", encoding="utf-8") as handle:
        rows = list(csv.DictReader(handle))

    indexed = {(row["operation"], row["implementation"]): row for row in rows}
    missing = [
        (operation, variant)
        for operation, _, _, variants in PREFETCH_OPERATIONS
        for variant, _ in variants
        if (operation, variant) not in indexed
    ]
    if missing:
        formatted = ", ".join(f"{op}/{variant}" for op, variant in missing)
        raise ValueError(f"Prefetch CSV is missing expected rows: {formatted}")
    return indexed


def nice_max(value: float) -> float:
    if value <= 0:
        return 1
    exponent = 10 ** math.floor(math.log10(value))
    fraction = value / exponent
    for candidate in (1, 1.5, 2, 2.5, 5, 7.5, 10):
        if fraction <= candidate:
            return candidate * exponent
    return 10 * exponent


def format_ns(value: float) -> str:
    if value >= 1000:
        return f"{value / 1000:.2f} μs"
    if value >= 100:
        return f"{value:.0f} ns"
    return f"{value:.1f} ns"


def draw_bar_figure(
    rows: dict[tuple[str, str], dict[str, str]],
    path: Path,
    scenario_title: str,
    source_path: Path,
) -> None:
    width, height = 1560, 650
    svg = Svg(width, height)
    svg.text(
        width / 2,
        38,
        f"{scenario_title}: median and p99 latency",
        size=25,
        weight="bold",
    )
    svg.text(
        width / 2,
        63,
        f"Values are read from {source_path}; each panel has its own scale",
        size=14,
        fill="#4B5563",
    )

    panel_lefts = (80, 580, 1080)
    panel_width = 410
    top, bottom = 118, 530
    plot_height = bottom - top
    bar_width = 27
    group_width = 92

    for panel_index, operation in enumerate(OPERATIONS):
        left = panel_lefts[panel_index]
        values = [
            float(rows[(operation, impl)][percentile])
            for impl in IMPLEMENTATIONS
            for percentile in ("p50_ns", "p99_ns")
        ]
        y_max = nice_max(max(values) * 1.12)

        svg.text(left + panel_width / 2, 98, OPERATION_LABELS[operation], size=18, weight="bold")
        for tick in range(6):
            value = y_max * tick / 5
            y = bottom - plot_height * tick / 5
            svg.line(left, y, left + panel_width, y, stroke="#D1D5DB", dash="4 4")
            svg.text(left - 10, y + 5, f"{value:g}", size=12, anchor="end", fill="#4B5563")
        svg.line(left, top, left, bottom, stroke="#374151", stroke_width=1.5)
        svg.line(left, bottom, left + panel_width, bottom, stroke="#374151", stroke_width=1.5)

        for impl_index, implementation in enumerate(IMPLEMENTATIONS):
            center = left + 48 + impl_index * group_width
            for metric_index, (metric, color) in enumerate(
                (("p50_ns", "#2563EB"), ("p99_ns", "#F97316"))
            ):
                value = float(rows[(operation, implementation)][metric])
                bar_height = plot_height * value / y_max
                x = center + (metric_index - 1) * bar_width + 2
                y = bottom - bar_height
                svg.rect(x, y, bar_width - 4, bar_height, fill=color, rx=2)
            svg.text(
                center + 2,
                bottom + 27,
                IMPLEMENTATION_LABELS[implementation],
                size=12,
                rotate=-28,
                anchor="end",
                fill="#374151",
            )

        if panel_index == 0:
            svg.text(left - 58, (top + bottom) / 2, "Latency (ns)", size=15, rotate=-90)

    legend_y = 614
    svg.rect(626, legend_y - 13, 18, 18, fill="#2563EB", rx=2)
    svg.text(652, legend_y + 2, "p50 (median)", size=14, anchor="start")
    svg.rect(782, legend_y - 13, 18, 18, fill="#F97316", rx=2)
    svg.text(808, legend_y + 2, "p99", size=14, anchor="start")
    svg.write(path)


def draw_percentile_figure(
    rows: dict[tuple[str, str], dict[str, str]],
    path: Path,
    scenario_title: str,
) -> None:
    width, height = 1560, 650
    svg = Svg(width, height)
    svg.text(
        width / 2,
        38,
        f"{scenario_title}: latency percentile profiles",
        size=25,
        weight="bold",
    )
    svg.text(
        width / 2,
        63,
        "Logarithmic latency axis; aggregate percentiles from the benchmark CSV",
        size=14,
        fill="#4B5563",
    )

    percentile_columns = ("p50_ns", "p95_ns", "p99_ns", "p999_ns", "p9999_ns")
    percentile_labels = ("p50", "p95", "p99", "p99.9", "p99.99")
    panel_lefts = (80, 580, 1080)
    panel_width = 410
    top, bottom = 118, 530
    plot_height = bottom - top

    for panel_index, operation in enumerate(OPERATIONS):
        left = panel_lefts[panel_index]
        all_values = [
            float(rows[(operation, impl)][column])
            for impl in IMPLEMENTATIONS
            for column in percentile_columns
        ]
        log_min = math.floor(math.log10(min(all_values)))
        log_max = math.ceil(math.log10(max(all_values)))
        if log_min == log_max:
            log_max += 1

        def y_for(value: float) -> float:
            position = (math.log10(value) - log_min) / (log_max - log_min)
            return bottom - position * plot_height

        svg.text(left + panel_width / 2, 98, OPERATION_LABELS[operation], size=18, weight="bold")
        for exponent in range(log_min, log_max + 1):
            value = 10**exponent
            y = y_for(value)
            svg.line(left, y, left + panel_width, y, stroke="#D1D5DB", dash="4 4")
            svg.text(left - 10, y + 5, format_ns(value), size=12, anchor="end", fill="#4B5563")
        svg.line(left, top, left, bottom, stroke="#374151", stroke_width=1.5)
        svg.line(left, bottom, left + panel_width, bottom, stroke="#374151", stroke_width=1.5)

        x_positions = [left + 28 + index * (panel_width - 56) / 4 for index in range(5)]
        for x, label in zip(x_positions, percentile_labels):
            svg.line(x, bottom, x, bottom + 5, stroke="#374151")
            svg.text(x, bottom + 24, label, size=12, fill="#374151")

        for implementation in IMPLEMENTATIONS:
            values = [float(rows[(operation, implementation)][column]) for column in percentile_columns]
            points = [(x, y_for(value)) for x, value in zip(x_positions, values)]
            svg.polyline(points, stroke=COLORS[implementation])
            for x, y in points:
                svg.circle(x, y, 4, fill=COLORS[implementation])

        if panel_index == 0:
            svg.text(left - 58, (top + bottom) / 2, "Latency (log scale)", size=15, rotate=-90)

    legend_x = 400
    legend_y = 614
    for index, implementation in enumerate(IMPLEMENTATIONS):
        x = legend_x + index * 210
        svg.line(x, legend_y - 4, x + 28, legend_y - 4, stroke=COLORS[implementation], stroke_width=4)
        svg.circle(x + 14, legend_y - 4, 4, fill=COLORS[implementation])
        svg.text(x + 37, legend_y + 1, IMPLEMENTATION_LABELS[implementation], size=14, anchor="start")
    svg.write(path)


def draw_uniform_model(path: Path) -> None:
    width, height = 1200, 560
    svg = Svg(width, height)
    svg.text(width / 2, 40, "Uniform price-distribution workload", size=25, weight="bold")
    svg.text(
        width / 2,
        66,
        "P ~ DiscreteUniform({1, …, 9,999}); Pr(P = p) = 1/9,999",
        size=16,
        fill="#374151",
    )

    left, right, top, bottom = 105, 1130, 125, 430
    plot_width = right - left
    plot_height = bottom - top
    expected_bin_share = 2.0
    y_max = 2.5

    for tick in range(6):
        value = y_max * tick / 5
        y = bottom - plot_height * value / y_max
        svg.line(left, y, right, y, stroke="#D1D5DB", dash="4 4")
        svg.text(left - 12, y + 5, f"{value:.1f}%", size=13, anchor="end", fill="#4B5563")
    svg.line(left, top, left, bottom, stroke="#374151", stroke_width=1.5)
    svg.line(left, bottom, right, bottom, stroke="#374151", stroke_width=1.5)

    bin_count = 50
    gap = 2
    bar_width = plot_width / bin_count
    bar_height = plot_height * expected_bin_share / y_max
    for index in range(bin_count):
        svg.rect(
            left + index * bar_width + gap / 2,
            bottom - bar_height,
            bar_width - gap,
            bar_height,
            fill="#60A5FA",
        )

    for value, label in ((1, "1"), (2500, "2,500"), (5000, "5,000"), (7500, "7,500"), (9999, "9,999")):
        x = left + (value - 1) / (9999 - 1) * plot_width
        svg.line(x, bottom, x, bottom + 6, stroke="#374151")
        svg.text(x, bottom + 28, label, size=13, fill="#374151")

    svg.text((left + right) / 2, 490, "Price tick", size=15)
    svg.text(30, (top + bottom) / 2, "Expected share per equal-width bin", size=15, rotate=-90)
    svg.text(
        width / 2,
        535,
        "The result CSV does not contain raw price draws; this is the theoretical workload model.",
        size=13,
        fill="#6B7280",
    )
    svg.write(path)


def draw_clustered_model(path: Path) -> None:
    width, height = 1200, 590
    svg = Svg(width, height)
    svg.text(width / 2, 40, "Clustered price-distribution workload", size=25, weight="bold")
    svg.text(
        width / 2,
        66,
        "90% uniform on ticks 4,990–5,010; 10% uniform on ticks 1–9,999",
        size=16,
        fill="#374151",
    )

    left, right, top, bottom = 105, 1130, 125, 450
    plot_width = right - left
    plot_height = bottom - top
    y_max = 100.0
    bin_count = 51
    central_bin = bin_count // 2
    background_share = 10.0 / bin_count
    central_share = 90.0 + background_share

    for tick in range(5):
        value = y_max * tick / 4
        y = bottom - plot_height * value / y_max
        svg.line(left, y, right, y, stroke="#D1D5DB", dash="4 4")
        svg.text(left - 12, y + 5, f"{value:.0f}%", size=13, anchor="end", fill="#4B5563")
    svg.line(left, top, left, bottom, stroke="#374151", stroke_width=1.5)
    svg.line(left, bottom, right, bottom, stroke="#374151", stroke_width=1.5)

    gap = 2
    bar_width = plot_width / bin_count
    for index in range(bin_count):
        value = central_share if index == central_bin else background_share
        bar_height = max(1.2, plot_height * value / y_max)
        svg.rect(
            left + index * bar_width + gap / 2,
            bottom - bar_height,
            bar_width - gap,
            bar_height,
            fill="#F97316" if index == central_bin else "#60A5FA",
        )

    center_x = left + (central_bin + 0.5) * bar_width
    center_y = bottom - plot_height * central_share / y_max
    svg.text(center_x + 18, center_y - 12, f"≈{central_share:.1f}%", size=14, anchor="start", weight="bold")
    svg.line(center_x, center_y - 5, center_x, top + 8, stroke="#F97316", stroke_width=1.5, dash="4 4")

    for value, label in ((1, "1"), (2500, "2,500"), (5000, "5,000"), (7500, "7,500"), (9999, "9,999")):
        x = left + (value - 1) / (9999 - 1) * plot_width
        svg.line(x, bottom, x, bottom + 6, stroke="#374151")
        svg.text(x, bottom + 28, label, size=13, fill="#374151")

    svg.text((left + right) / 2, 510, "Price tick", size=15)
    svg.text(30, (top + bottom) / 2, "Expected share per equal-width bin", size=15, rotate=-90)
    svg.rect(365, 535, 18, 18, fill="#F97316", rx=2)
    svg.text(393, 549, "Bin containing the 21-tick cluster", size=13, anchor="start")
    svg.rect(690, 535, 18, 18, fill="#60A5FA", rx=2)
    svg.text(718, 549, "Full-range background", size=13, anchor="start")
    svg.text(
        width / 2,
        580,
        "The result CSV does not contain raw price draws; this is the theoretical mixture model.",
        size=13,
        fill="#6B7280",
    )
    svg.write(path)


def draw_zipfian_model(path: Path) -> None:
    width, height = 1200, 610
    svg = Svg(width, height)
    svg.text(width / 2, 40, "Zipfian price-distribution workload", size=25, weight="bold")
    svg.text(
        width / 2,
        66,
        "Expected distribution in a 10,000-order book; 200 price levels and exponent s = 1",
        size=16,
        fill="#374151",
    )

    left, right, top, bottom = 105, 1130, 125, 450
    plot_width = right - left
    plot_height = bottom - top
    harmonic_200 = sum(1.0 / rank for rank in range(1, 201))
    min_price, max_price = 4_901, 5_100
    y_max = 1_800.0

    for value in range(0, 1_801, 300):
        y = bottom - plot_height * value / y_max
        svg.line(left, y, right, y, stroke="#D1D5DB", dash="4 4")
        svg.text(left - 12, y + 5, f"{value:,}", size=13, anchor="end", fill="#4B5563")

    svg.line(left, top, left, bottom, stroke="#374151", stroke_width=1.5)
    svg.line(left, bottom, right, bottom, stroke="#374151", stroke_width=1.5)

    expected_by_price: dict[int, tuple[int, float]] = {}
    for rank in range(1, 201):
        if rank == 1:
            price = 5_000
        elif rank % 2 == 0:
            price = 5_000 + rank // 2
        else:
            price = 5_000 - rank // 2
        expected_orders = 10_000.0 / (rank * harmonic_200)
        expected_by_price[price] = (rank, expected_orders)

    bar_width = plot_width / len(expected_by_price)
    for price in range(min_price, max_price + 1):
        rank, expected_orders = expected_by_price[price]
        bar_height = plot_height * expected_orders / y_max
        x = left + (price - min_price) * bar_width
        svg.rect(
            x + 0.35,
            bottom - bar_height,
            max(1.0, bar_width - 0.7),
            bar_height,
            fill="#5B21B6" if rank == 1 else "#A78BFA",
        )

    for price, label in ((4_901, "4,901"), (4_950, "4,950"), (5_000, "5,000"), (5_050, "5,050"), (5_100, "5,100")):
        x = left + ((price - min_price) + 0.5) * bar_width
        svg.line(x, bottom, x, bottom + 6, stroke="#374151")
        svg.text(x, bottom + 28, label, size=13, fill="#374151")

    svg.text((left + right) / 2, 505, "Price tick", size=15)
    svg.text(30, (top + bottom) / 2, "Expected orders per 10,000", size=15, rotate=-90)
    svg.rect(370, 530, 18, 18, fill="#5B21B6", rx=2)
    svg.text(398, 544, "Rank 1: price 5,000 (≈1,701 orders)", size=13, anchor="start")
    svg.rect(690, 530, 18, 18, fill="#A78BFA", rx=2)
    svg.text(718, 544, "Ranks 2–200", size=13, anchor="start")
    svg.text(
        width / 2,
        585,
        "The result CSV does not contain raw price draws; these bars show the theoretical expected counts.",
        size=13,
        fill="#6B7280",
    )
    svg.write(path)


def draw_bursty_model(path: Path) -> None:
    width, height = 1200, 780
    svg = Svg(width, height)
    svg.text(width / 2, 40, "Bursty price-distribution workload", size=25, weight="bold")
    svg.text(
        width / 2,
        66,
        "10 × [500-order local burst → 50-order wide quiet phase]; no artificial time delays",
        size=16,
        fill="#374151",
    )

    left, right = 105, 1130
    plot_width = right - left
    burst_color = "#F97316"
    quiet_color = "#60A5FA"

    # The upper panel preserves the order in which the insertion workload is
    # generated. Segment widths are proportional to the number of operations.
    timeline_top, timeline_bottom = 125, 205
    svg.text(left, 106, "Insertion sequence in one 5,500-order book", size=16, anchor="start", weight="bold")
    cycle_width = plot_width / 10
    burst_width = cycle_width * 500 / 550
    quiet_width = cycle_width * 50 / 550
    for cycle in range(10):
        x = left + cycle * cycle_width
        svg.rect(x, timeline_top, burst_width, timeline_bottom - timeline_top, fill=burst_color)
        svg.rect(
            x + burst_width,
            timeline_top,
            quiet_width,
            timeline_bottom - timeline_top,
            fill=quiet_color,
        )
        svg.line(x, timeline_top, x, timeline_bottom, stroke="#FFFFFF", stroke_width=1)
        svg.text(x + cycle_width / 2, timeline_bottom + 23, str(cycle + 1), size=12, fill="#374151")
    svg.line(right, timeline_top, right, timeline_bottom, stroke="#FFFFFF", stroke_width=1)
    svg.text((left + right) / 2, 252, "Burst/quiet cycle", size=14)
    svg.rect(340, 268, 18, 18, fill=burst_color, rx=2)
    svg.text(368, 282, "Burst: 500 orders over 20 ticks", size=13, anchor="start")
    svg.rect(675, 268, 18, 18, fill=quiet_color, rx=2)
    svg.text(703, 282, "Quiet: 50 orders over 2,000 ticks", size=13, anchor="start")

    # The lower panel aggregates the ten temporal cycles into equal 20-tick
    # price bins. Every bin receives five expected quiet orders; burst windows
    # add their concentrated mass while their centers drift from 5,000 to 5,090.
    dist_top, dist_bottom = 345, 650
    dist_height = dist_bottom - dist_top
    svg.text(left, 326, "Expected aggregate price distribution", size=16, anchor="start", weight="bold")
    y_max = 1_100.0
    for value in (0, 250, 500, 750, 1_000):
        y = dist_bottom - dist_height * value / y_max
        svg.line(left, y, right, y, stroke="#D1D5DB", dash="4 4")
        svg.text(left - 12, y + 5, f"{value:,}", size=13, anchor="end", fill="#4B5563")
    svg.line(left, dist_top, left, dist_bottom, stroke="#374151", stroke_width=1.5)
    svg.line(left, dist_bottom, right, dist_bottom, stroke="#374151", stroke_width=1.5)

    bin_min = 4_000
    bin_width_ticks = 20
    bin_count = 100
    bar_width = plot_width / bin_count
    expected_counts = [5.0 for _ in range(bin_count)]
    for cycle in range(10):
        burst_min = 4_990 + cycle * 10
        burst_max = burst_min + 19
        for price in range(burst_min, burst_max + 1):
            bin_index = (price - bin_min) // bin_width_ticks
            expected_counts[bin_index] += 25.0

    for index, expected_orders in enumerate(expected_counts):
        bar_height = max(1.3, dist_height * expected_orders / y_max)
        svg.rect(
            left + index * bar_width + 0.8,
            dist_bottom - bar_height,
            bar_width - 1.6,
            bar_height,
            fill=burst_color if expected_orders > 5.0 else quiet_color,
        )

    for price, label in ((4_000, "4,000"), (4_500, "4,500"), (5_000, "5,000"), (5_500, "5,500"), (5_999, "5,999")):
        x = left + (price - 4_000) / (5_999 - 4_000) * plot_width
        svg.line(x, dist_bottom, x, dist_bottom + 6, stroke="#374151")
        svg.text(x, dist_bottom + 27, label, size=13, fill="#374151")

    svg.text((left + right) / 2, 708, "Price tick", size=15)
    svg.text(30, (dist_top + dist_bottom) / 2, "Expected orders per 20-tick bin", size=15, rotate=-90)
    svg.text(
        width / 2,
        758,
        "Orange includes burst and quiet orders; blue is the five-order quiet-phase baseline per bin.",
        size=13,
        fill="#6B7280",
    )
    svg.write(path)


def draw_high_cancel_model(path: Path) -> None:
    width, height = 1200, 650
    svg = Svg(width, height)
    add_color = "#2563EB"
    cancel_color = "#F97316"
    market_color = "#059669"

    svg.text(width / 2, 40, "High-cancellation operational workload", size=25, weight="bold")
    svg.text(
        width / 2,
        66,
        "One round: 110 additions → 100 cancellations → 10 successful market orders → empty book",
        size=16,
        fill="#374151",
    )

    box_y, box_height, box_width = 125, 130, 300
    boxes = (
        (75, add_color, "1. Add 110 orders", "55 bids + 55 asks", "Bids 4,975–4,999; asks 5,000–5,024"),
        (450, cancel_color, "2. Cancel 100 orders", "50 bids + 50 asks", "Random order; five survivors per side"),
        (825, market_color, "3. Execute 10 orders", "5 market buys + 5 market sells", "Each consumes one 100-unit survivor"),
    )
    for x, color, heading, detail, note in boxes:
        svg.rect(x, box_y, box_width, box_height, fill="#FFFFFF", stroke=color, stroke_width=3, rx=8)
        svg.rect(x, box_y, box_width, 38, fill=color, rx=7)
        svg.text(x + box_width / 2, box_y + 26, heading, size=17, weight="bold", fill="#FFFFFF")
        svg.text(x + box_width / 2, box_y + 76, detail, size=15, weight="bold")
        svg.text(x + box_width / 2, box_y + 105, note, size=12, fill="#4B5563")
    svg.text(412, box_y + 72, "→", size=34, weight="bold", fill="#6B7280")
    svg.text(787, box_y + 72, "→", size=34, weight="bold", fill="#6B7280")

    svg.text(100, 326, "Measured operation mix per round", size=17, anchor="start", weight="bold")
    bar_left, bar_right, bar_top, bar_height = 100, 1100, 350, 78
    total_operations = 220
    segments = (
        (110, add_color, "110 adds"),
        (100, cancel_color, "100 cancels"),
        (10, market_color, "10 market"),
    )
    x = bar_left
    for count, color, label in segments:
        segment_width = (bar_right - bar_left) * count / total_operations
        svg.rect(x, bar_top, segment_width, bar_height, fill=color, stroke="#FFFFFF", stroke_width=1)
        if segment_width >= 100:
            svg.text(x + segment_width / 2, bar_top + 47, label, size=16, weight="bold", fill="#FFFFFF")
        x += segment_width

    legend_y = 478
    svg.rect(295, legend_y - 14, 18, 18, fill=add_color, rx=2)
    svg.text(323, legend_y, "50.0% additions", size=14, anchor="start")
    svg.rect(505, legend_y - 14, 18, 18, fill=cancel_color, rx=2)
    svg.text(533, legend_y, "45.5% cancellations", size=14, anchor="start")
    svg.rect(735, legend_y - 14, 18, 18, fill=market_color, rx=2)
    svg.text(763, legend_y, "4.5% market orders", size=14, anchor="start")

    svg.text(
        width / 2,
        548,
        "Cancellation-to-trade ratio: 100 cancelled resting orders / 10 traded resting orders = 10:1",
        size=15,
        weight="bold",
    )
    svg.text(
        width / 2,
        580,
        "All prices lie inside the hybrid hot zone [4,900, 5,100); each order has quantity 100.",
        size=14,
        fill="#374151",
    )
    svg.text(
        width / 2,
        620,
        "Each book batch uses 10 unmeasured warm-up rounds followed by 100 measured rounds.",
        size=13,
        fill="#6B7280",
    )
    svg.write(path)


def draw_sweep_model(path: Path) -> None:
    width, height = 1200, 700
    svg = Svg(width, height)
    svg.text(width / 2, 40, "Market-order sweep workload", size=25, weight="bold")
    svg.text(
        width / 2,
        66,
        "Dense 400-level book; one 100-unit order at every price level",
        size=16,
        fill="#374151",
    )

    left, right = 220, 1120
    price_min, price_max = 4_800, 5_199
    plot_width = right - left

    def x_for(price: float) -> float:
        return left + (price - price_min) / (price_max - price_min) * plot_width

    hot_left, hot_right = x_for(4_900), x_for(5_100)
    svg.rect(hot_left, 100, hot_right - hot_left, 430, fill="#ECFDF5", opacity=0.85)
    svg.text((hot_left + hot_right) / 2, 116, "Hybrid hot zone [4,900, 5,100)", size=13, weight="bold", fill="#047857")

    book_top, book_height = 135, 46
    center_x = x_for(4_999.5)
    svg.rect(x_for(4_800), book_top, center_x - x_for(4_800), book_height, fill="#60A5FA")
    svg.rect(center_x, book_top, x_for(5_199) - center_x, book_height, fill="#FB923C")
    svg.text((x_for(4_800) + center_x) / 2, book_top + 29, "200 bid levels: 4,999 down to 4,800", size=14, weight="bold", fill="#FFFFFF")
    svg.text((center_x + x_for(5_199)) / 2, book_top + 29, "200 ask levels: 5,000 up to 5,199", size=14, weight="bold", fill="#FFFFFF")

    for price, label in ((4_800, "4,800"), (4_900, "4,900"), (4_999.5, "4,999 | 5,000"), (5_100, "5,100"), (5_199, "5,199")):
        x = x_for(price)
        svg.line(x, book_top + book_height, x, 530, stroke="#9CA3AF", dash="4 4")
        svg.text(x, book_top + book_height + 22, label, size=12, fill="#374151")

    svg.text((left + center_x) / 2, 228, "← Market sell consumes bids", size=14, weight="bold", fill="#1D4ED8")
    svg.text((center_x + right) / 2, 228, "Market buy consumes asks →", size=14, weight="bold", fill="#C2410C")

    hot_color = "#10B981"
    cold_color = "#F97316"
    row_y_values = (260, 330, 400, 470)
    for (case, levels), row_y in zip(SWEEP_CASES, row_y_values):
        quantity = levels * 100
        svg.text(
            left - 14,
            row_y + 20,
            f"{levels} levels / {quantity:,} units",
            size=13,
            anchor="end",
            weight="bold",
        )
        sell_start = 5_000 - levels
        buy_end = 5_000 + levels

        sell_hot_start = max(sell_start, 4_900)
        buy_hot_end = min(buy_end, 5_100)
        svg.rect(x_for(sell_hot_start), row_y, center_x - x_for(sell_hot_start), 32, fill=hot_color, rx=3)
        svg.rect(center_x, row_y, x_for(buy_hot_end) - center_x, 32, fill=hot_color, rx=3)

        if sell_start < 4_900:
            svg.rect(x_for(sell_start), row_y, x_for(4_900) - x_for(sell_start), 32, fill=cold_color, rx=3)
            svg.text((x_for(sell_start) + x_for(4_900)) / 2, row_y + 21, "50 cold", size=12, weight="bold", fill="#FFFFFF")
        if buy_end > 5_100:
            svg.rect(x_for(5_100), row_y, x_for(buy_end) - x_for(5_100), 32, fill=cold_color, rx=3)
            svg.text((x_for(5_100) + x_for(buy_end)) / 2, row_y + 21, "50 cold", size=12, weight="bold", fill="#FFFFFF")

    svg.rect(375, 558, 18, 18, fill=hot_color, rx=2)
    svg.text(403, 572, "Levels in the hybrid hot array", size=13, anchor="start")
    svg.rect(660, 558, 18, 18, fill=cold_color, rx=2)
    svg.text(688, 572, "Levels in the hybrid cold B-tree", size=13, anchor="start")
    svg.text(
        width / 2,
        620,
        "The 5-, 20-, and 50-level sweeps remain hot; the 150-level sweep consumes 100 hot and 50 cold levels.",
        size=14,
        fill="#374151",
    )
    svg.text(
        width / 2,
        660,
        "Only execute_market_order is timed; validation, fill destruction, and full replenishment are untimed.",
        size=13,
        fill="#6B7280",
    )
    svg.write(path)


def draw_sweep_latency_figure(
    rows: dict[tuple[str, str], dict[str, str]],
    path: Path,
    metric: str,
) -> None:
    width, height = 1560, 680
    svg = Svg(width, height)
    metric_label = "median" if metric == "p50_ns" else "p99"
    svg.text(width / 2, 38, f"Market-sweep {metric_label} latency versus depth", size=25, weight="bold")
    svg.text(
        width / 2,
        63,
        "One million measurements per point; logarithmic latency axis; the final point crosses the hybrid boundary",
        size=14,
        fill="#4B5563",
    )

    panel_lefts = (100, 830)
    panel_width = 620
    top, bottom = 120, 550
    plot_height = bottom - top
    y_min, y_max = 300.0, 20_000.0
    y_ticks = (500.0, 1_000.0, 2_000.0, 5_000.0, 10_000.0, 20_000.0)

    def y_for(value: float) -> float:
        position = math.log10(value / y_min) / math.log10(y_max / y_min)
        return bottom - position * plot_height

    for panel_index, direction in enumerate(SWEEP_DIRECTIONS):
        left = panel_lefts[panel_index]
        x_positions = [left + 55 + index * (panel_width - 110) / 3 for index in range(4)]
        cross_boundary_x = (x_positions[2] + x_positions[3]) / 2
        svg.rect(
            cross_boundary_x,
            top,
            left + panel_width - cross_boundary_x,
            plot_height,
            fill="#FFF7ED",
            opacity=0.8,
        )
        svg.text(
            (cross_boundary_x + left + panel_width) / 2,
            top + 19,
            "hot/cold case",
            size=12,
            fill="#C2410C",
            weight="bold",
        )
        svg.text(
            left + panel_width / 2,
            101,
            "Market buy (consumes asks)" if direction == "buy" else "Market sell (consumes bids)",
            size=18,
            weight="bold",
        )
        for tick in y_ticks:
            y = y_for(tick)
            svg.line(left, y, left + panel_width, y, stroke="#D1D5DB", dash="4 4")
            svg.text(left - 10, y + 5, format_ns(tick), size=12, anchor="end", fill="#4B5563")
        svg.line(left, top, left, bottom, stroke="#374151", stroke_width=1.5)
        svg.line(left, bottom, left + panel_width, bottom, stroke="#374151", stroke_width=1.5)

        for x, (_, levels) in zip(x_positions, SWEEP_CASES):
            svg.line(x, bottom, x, bottom + 5, stroke="#374151")
            svg.text(x, bottom + 25, str(levels), size=13, fill="#374151")

        for implementation in IMPLEMENTATIONS:
            values = [
                float(rows[(f"{case}_{direction}_sweep", implementation)][metric])
                for case, _ in SWEEP_CASES
            ]
            points = [(x, y_for(value)) for x, value in zip(x_positions, values)]
            svg.polyline(points, stroke=COLORS[implementation])
            for x, y in points:
                svg.circle(x, y, 5, fill=COLORS[implementation])

        svg.text(left + panel_width / 2, 600, "Price levels consumed", size=14)
        if panel_index == 0:
            svg.text(left - 67, (top + bottom) / 2, "Latency (log scale)", size=15, rotate=-90)

    legend_x, legend_y = 380, 652
    for index, implementation in enumerate(IMPLEMENTATIONS):
        x = legend_x + index * 215
        svg.line(x, legend_y - 4, x + 28, legend_y - 4, stroke=COLORS[implementation], stroke_width=4)
        svg.circle(x + 14, legend_y - 4, 4, fill=COLORS[implementation])
        svg.text(x + 37, legend_y + 1, IMPLEMENTATION_LABELS[implementation], size=14, anchor="start")
    svg.write(path)


def draw_buildup_model(path: Path) -> None:
    width, height = 1200, 700
    svg = Svg(width, height)
    measured_color = "#2563EB"
    prefill_color = "#D1D5DB"
    bid_color = "#60A5FA"
    ask_color = "#FB923C"
    hot_color = "#10B981"

    svg.text(width / 2, 40, "Order-book build-up workload", size=25, weight="bold")
    svg.text(
        width / 2,
        66,
        "Five 500-addition measurement windows in each fresh-book lifecycle",
        size=16,
        fill="#374151",
    )

    left, right = 100, 1120
    timeline_top, timeline_height = 155, 54
    total_depth = 10_500

    def depth_x(depth: float) -> float:
        return left + depth / total_depth * (right - left)

    svg.text(left, 112, "Book-depth progression", size=17, anchor="start", weight="bold")
    svg.rect(left, timeline_top, right - left, timeline_height, fill=prefill_color, rx=4)
    for index, (_, start, end) in enumerate(BUILDUP_WINDOWS):
        x1 = depth_x(start)
        x2 = depth_x(end + 1)
        svg.rect(x1, timeline_top, x2 - x1, timeline_height, fill=measured_color)
        label_y = 135 if index % 2 == 0 else 238
        connector_y = timeline_top if index % 2 == 0 else timeline_top + timeline_height
        connector_end_y = label_y + 8 if index % 2 == 0 else label_y - 15
        svg.line((x1 + x2) / 2, connector_y, (x1 + x2) / 2, connector_end_y, stroke=measured_color)
        svg.text(
            (x1 + x2) / 2,
            label_y,
            f"{start:,}–{end:,}",
            size=12,
            weight="bold",
            fill="#1D4ED8",
        )

    svg.text(left, 273, "0", size=12, anchor="start", fill="#4B5563")
    svg.text(right, 273, "10,500 resting orders", size=12, anchor="end", fill="#4B5563")
    svg.rect(335, 292, 18, 18, fill=measured_color, rx=2)
    svg.text(363, 306, "Timed add_order calls", size=13, anchor="start")
    svg.rect(600, 292, 18, 18, fill=prefill_color, rx=2)
    svg.text(628, 306, "Untimed prefill", size=13, anchor="start")
    svg.text(
        width / 2,
        342,
        "Book construction is untimed; the first blue window begins immediately in the new empty book.",
        size=13,
        fill="#6B7280",
    )

    price_top, price_height = 430, 66
    price_min, split_price, price_max = 1, 5_000, 10_000
    plot_width = right - left

    def price_x(price: float) -> float:
        return left + (price - price_min) / (price_max - price_min) * plot_width

    svg.text(left, 395, "Price and side model", size=17, anchor="start", weight="bold")
    split_x = price_x(split_price)
    svg.rect(left, price_top, split_x - left, price_height, fill=bid_color, rx=3)
    svg.rect(split_x, price_top, right - split_x, price_height, fill=ask_color, rx=3)
    svg.text((left + split_x) / 2, price_top + 40, "Bids: uniform on ticks 1–4,999", size=15, weight="bold", fill="#FFFFFF")
    svg.text((split_x + right) / 2, price_top + 40, "Asks: uniform on ticks 5,000–9,999", size=15, weight="bold", fill="#FFFFFF")

    hot_left, hot_right = price_x(4_900), price_x(5_100)
    svg.rect(hot_left, price_top - 7, hot_right - hot_left, price_height + 14, fill=hot_color, opacity=0.9)
    svg.line((hot_left + hot_right) / 2, price_top - 7, (hot_left + hot_right) / 2, 390, stroke="#047857")
    svg.text(
        (hot_left + hot_right) / 2,
        378,
        "Hybrid hot zone [4,900, 5,100)",
        size=12,
        weight="bold",
        fill="#047857",
    )

    for price, label in ((1, "1"), (5_000, "5,000"), (9_999, "9,999")):
        x = price_x(price)
        svg.line(x, price_top + price_height, x, price_top + price_height + 6, stroke="#374151")
        svg.text(x, price_top + price_height + 25, label, size=12, fill="#374151")

    svg.text(
        width / 2,
        570,
        "Each complete window alternates 250 bids and 250 asks; every order has quantity 100.",
        size=14,
        fill="#374151",
    )
    svg.text(
        width / 2,
        604,
        "Expected hybrid routing: approximately 2% hot-array additions and 98% cold-tree additions.",
        size=14,
        weight="bold",
    )
    svg.text(
        width / 2,
        652,
        "RNG, order construction, prefill, result checking, and final validation are outside the timed interval.",
        size=13,
        fill="#6B7280",
    )
    svg.write(path)


def draw_buildup_latency_figure(
    rows: dict[tuple[str, str], dict[str, str]],
    path: Path,
    metric: str,
) -> None:
    width, height = 1200, 670
    svg = Svg(width, height)
    metric_label = "median" if metric == "p50_ns" else "p99"
    svg.text(width / 2, 40, f"Build-up {metric_label} insertion latency", size=25, weight="bold")
    svg.text(
        width / 2,
        66,
        "One million measured additions per depth window; latency derived from calibrated TSC ticks",
        size=14,
        fill="#4B5563",
    )

    left, right, top, bottom = 105, 1130, 120, 520
    plot_width = right - left
    plot_height = bottom - top
    all_values = [
        float(rows[(operation, implementation)][metric])
        for operation, _, _ in BUILDUP_WINDOWS
        for implementation in IMPLEMENTATIONS
    ]
    y_max = nice_max(max(all_values) * 1.15)

    def y_for(value: float) -> float:
        return bottom - value / y_max * plot_height

    x_positions = [left + index * plot_width / 4 for index in range(5)]
    svg.rect(left, top, (x_positions[0] + x_positions[1]) / 2 - left, plot_height, fill="#EFF6FF")
    svg.text(left + 58, top + 20, "fresh-book window", size=12, fill="#1D4ED8", weight="bold")

    for tick in range(6):
        value = y_max * tick / 5
        y = y_for(value)
        svg.line(left, y, right, y, stroke="#D1D5DB", dash="4 4")
        svg.text(left - 12, y + 5, f"{value:g}", size=12, anchor="end", fill="#4B5563")
    svg.line(left, top, left, bottom, stroke="#374151", stroke_width=1.5)
    svg.line(left, bottom, right, bottom, stroke="#374151", stroke_width=1.5)

    for x, (_, start, end) in zip(x_positions, BUILDUP_WINDOWS):
        svg.line(x, bottom, x, bottom + 6, stroke="#374151")
        svg.text(x, bottom + 25, f"{start:,}", size=13, fill="#374151")
        svg.text(x, bottom + 44, f"({start:,}–{end:,})", size=11, fill="#6B7280")

    for implementation in IMPLEMENTATIONS:
        values = [
            float(rows[(operation, implementation)][metric])
            for operation, _, _ in BUILDUP_WINDOWS
        ]
        points = [(x, y_for(value)) for x, value in zip(x_positions, values)]
        svg.polyline(points, stroke=COLORS[implementation])
        for x, y in points:
            svg.circle(x, y, 5, fill=COLORS[implementation])

    svg.text((left + right) / 2, 590, "Starting book depth; measured window shown in parentheses", size=14)
    svg.text(32, (top + bottom) / 2, "Latency (ns)", size=15, rotate=-90)

    legend_x, legend_y = 205, 640
    for index, implementation in enumerate(IMPLEMENTATIONS):
        x = legend_x + index * 235
        svg.line(x, legend_y - 4, x + 28, legend_y - 4, stroke=COLORS[implementation], stroke_width=4)
        svg.circle(x + 14, legend_y - 4, 4, fill=COLORS[implementation])
        svg.text(x + 37, legend_y + 1, IMPLEMENTATION_LABELS[implementation], size=14, anchor="start")
    svg.write(path)


def draw_alignment_model(path: Path) -> None:
    width, height = 1200, 780
    svg = Svg(width, height)
    svg.text(width / 2, 40, "Alignment and padding layouts", size=25, weight="bold")
    svg.text(
        width / 2,
        66,
        "Record boundaries across two 64-byte cache lines",
        size=16,
        fill="#374151",
    )

    left, right = 240, 1120
    line_width = (right - left) / 2

    def byte_x(offset: float) -> float:
        return left + offset / 128 * (right - left)

    svg.text(left + line_width / 2, 108, "Cache line 0: bytes 0–63", size=14, weight="bold")
    svg.text(left + line_width + line_width / 2, 108, "Cache line 1: bytes 64–127", size=14, weight="bold")

    layouts = (
        ("Default", 24, 8, "24 B / alignment 8 B", "25% theoretical straddles"),
        ("Packed", 17, 1, "17 B / alignment 1 B", "25% theoretical straddles"),
        ("Aligned", 64, 64, "64 B / alignment 64 B", "0% straddles"),
    )
    row_y_values = (140, 260, 380)
    colors = ("#60A5FA", "#FB923C", "#A78BFA")

    for (name, record_size, _, size_label, straddle_label), row_y, color in zip(
        layouts, row_y_values, colors
    ):
        svg.text(left - 20, row_y + 23, name, size=17, anchor="end", weight="bold")
        svg.text(left - 20, row_y + 47, size_label, size=12, anchor="end", fill="#4B5563")
        record_index = 1
        offset = 0
        while offset < 128:
            record_end = min(offset + record_size, 128)
            straddles = offset < 64 < offset + record_size
            fill = "#DC2626" if straddles else color
            x1, x2 = byte_x(offset), byte_x(record_end)
            svg.rect(x1, row_y, x2 - x1, 64, fill=fill, stroke="#FFFFFF", stroke_width=1)
            if x2 - x1 >= 42:
                svg.text(
                    (x1 + x2) / 2,
                    row_y + 39,
                    f"O{record_index}",
                    size=13,
                    weight="bold",
                    fill="#FFFFFF",
                )
            offset += record_size
            record_index += 1
        svg.text(right, row_y + 84, straddle_label, size=12, anchor="end", fill="#4B5563")

    boundary_x = byte_x(64)
    svg.line(boundary_x, 120, boundary_x, 466, stroke="#991B1B", stroke_width=2, dash="5 4")
    svg.text(boundary_x, 490, "64-byte boundary", size=12, weight="bold", fill="#991B1B")
    svg.rect(315, 474, 18, 18, fill="#DC2626", rx=2)
    svg.text(343, 488, "Record crossing the boundary", size=12, anchor="start")
    svg.text(
        840,
        516,
        "Boxes show record boundaries, not internal field offsets.",
        size=12,
        fill="#6B7280",
    )

    svg.text(90, 540, "Footprint of a 10,000-record Vec", size=17, anchor="start", weight="bold")
    footprint_left, footprint_right = 330, 1080
    footprint_max = 640_000
    footprints = (
        ("Default", 240_000, "234.4 KiB", colors[0]),
        ("Packed", 170_000, "166.0 KiB", colors[1]),
        ("Aligned", 640_000, "625.0 KiB", colors[2]),
    )
    for index, (name, byte_count, label, color) in enumerate(footprints):
        y = 565 + index * 52
        bar_width = (footprint_right - footprint_left) * byte_count / footprint_max
        svg.text(footprint_left - 16, y + 22, name, size=14, anchor="end", weight="bold")
        svg.rect(footprint_left, y, bar_width, 30, fill=color, rx=3)
        svg.text(footprint_left + bar_width + 12, y + 21, label, size=13, anchor="start")

    svg.text(
        width / 2,
        750,
        "The straddle calculation assumes a cache-line-aligned vector base; false sharing is not measured.",
        size=13,
        fill="#6B7280",
    )
    svg.write(path)


def draw_alignment_latency_figure(
    rows: dict[tuple[str, str], dict[str, str]],
    path: Path,
    metric: str,
) -> None:
    width, height = 1350, 670
    svg = Svg(width, height)
    metric_label = "median" if metric == "p50_ns" else "p99"
    svg.text(width / 2, 40, f"Alignment experiment: {metric_label} batch latency", size=25, weight="bold")
    svg.text(
        width / 2,
        66,
        "1,000 samples per bar; each panel has its own scale and batch definition",
        size=14,
        fill="#4B5563",
    )

    panel_lefts = (75, 505, 935)
    panel_width = 340
    top, bottom = 130, 510
    plot_height = bottom - top
    short_labels = (("Default", "24 B"), ("Packed", "17 B"), ("Aligned", "64 B"))

    for panel_index, (operation, operation_label, batch_label) in enumerate(ALIGNMENT_OPERATIONS):
        left = panel_lefts[panel_index]
        values = [float(rows[(operation, layout)][metric]) for layout in ALIGNMENT_LAYOUTS]
        y_max = nice_max(max(values) * 1.15)

        svg.text(left + panel_width / 2, 102, operation_label, size=18, weight="bold")
        svg.text(left + panel_width / 2, 122, f"Timed batch: {batch_label}", size=12, fill="#4B5563")
        for tick in range(6):
            value = y_max * tick / 5
            y = bottom - plot_height * value / y_max
            svg.line(left, y, left + panel_width, y, stroke="#D1D5DB", dash="4 4")
            svg.text(left - 9, y + 5, format_ns(value), size=11, anchor="end", fill="#4B5563")
        svg.line(left, top, left, bottom, stroke="#374151", stroke_width=1.5)
        svg.line(left, bottom, left + panel_width, bottom, stroke="#374151", stroke_width=1.5)

        for index, (layout, value, (short_name, size_label)) in enumerate(
            zip(ALIGNMENT_LAYOUTS, values, short_labels)
        ):
            center = left + 66 + index * 105
            bar_width = 58
            bar_height = plot_height * value / y_max
            svg.rect(
                center - bar_width / 2,
                bottom - bar_height,
                bar_width,
                bar_height,
                fill=ALIGNMENT_COLORS[layout],
                rx=3,
            )
            svg.text(center, bottom - bar_height - 9, format_ns(value), size=11, weight="bold")
            svg.text(center, bottom + 24, short_name, size=12, weight="bold")
            svg.text(center, bottom + 42, size_label, size=11, fill="#4B5563")

        if panel_index == 0:
            svg.text(left - 61, (top + bottom) / 2, "Batch latency", size=14, rotate=-90)

    svg.text(
        width / 2,
        608,
        "Bars are comparable within a panel; operation panels use different work per timed sample.",
        size=14,
        fill="#374151",
    )
    svg.text(
        width / 2,
        640,
        "Nanoseconds are derived from the calibrated TSC frequency recorded in results/bench_alignment.csv.",
        size=12,
        fill="#6B7280",
    )
    svg.write(path)


def draw_prefetch_model(path: Path) -> None:
    width, height = 1300, 850
    svg = Svg(width, height)
    baseline_color = "#2563EB"
    near_color = "#F97316"
    far_color = "#7C3AED"
    header_color = "#D1D5DB"
    heap_color = "#A7F3D0"

    svg.text(width / 2, 40, "Software-prefetch experiment", size=25, weight="bold")
    svg.text(
        width / 2,
        66,
        "Four timed batches using the x86 T0 high-temporal-locality hint",
        size=16,
        fill="#374151",
    )

    card_left, card_width, card_height = 75, 1150, 155
    card_y_values = (100, 280, 460, 640)
    for y in card_y_values:
        svg.rect(card_left, y, card_width, card_height, fill="#F9FAFB", stroke="#D1D5DB", rx=8)

    # Sequential header scan.
    y = card_y_values[0]
    svg.text(100, y + 28, "1. Sequential scan", size=17, anchor="start", weight="bold")
    svg.text(100, y + 52, "Read Vec::len for all 10,000 contiguous level headers", size=13, anchor="start", fill="#4B5563")
    cells_left, cell_y, cell_width = 460, y + 48, 43
    for index in range(17):
        fill = header_color
        if index == 1:
            fill = baseline_color
        elif index == 5:
            fill = near_color
        elif index == 16:
            fill = far_color
        svg.rect(cells_left + index * cell_width, cell_y, cell_width - 3, 38, fill=fill, stroke="#FFFFFF")
    svg.text(cells_left + 1.5 * cell_width, y + 112, "current i", size=12, fill="#1D4ED8")
    svg.text(cells_left + 5.5 * cell_width, y + 132, "hint i + 4", size=12, fill="#C2410C")
    svg.text(cells_left + 16.5 * cell_width, y + 112, "hint i + 16", size=12, fill="#6D28D9")

    # Random header access with a known index stream.
    y = card_y_values[1]
    svg.text(100, y + 28, "2. Known random access", size=17, anchor="start", weight="bold")
    svg.text(100, y + 52, "Read 10,000 pre-generated random level indices", size=13, anchor="start", fill="#4B5563")
    boxes = (
        (470, baseline_color, "index i", "current header"),
        (735, near_color, "index i + 1", "near hint"),
        (1000, far_color, "index i + 4", "far hint"),
    )
    for x, color, heading, detail in boxes:
        svg.rect(x, y + 48, 165, 62, fill="#FFFFFF", stroke=color, stroke_width=3, rx=6)
        svg.text(x + 82.5, y + 73, heading, size=14, weight="bold", fill=color)
        svg.text(x + 82.5, y + 96, detail, size=12, fill="#4B5563")
    svg.text(682, y + 87, "→", size=28, fill="#6B7280")
    svg.text(947, y + 87, "→", size=28, fill="#6B7280")
    svg.text(817, y + 135, "Future addresses are known from the index array.", size=12, fill="#6B7280")

    # Sparse pointer chase.
    y = card_y_values[2]
    svg.text(100, y + 28, "3. Sparse pointer chase", size=17, anchor="start", weight="bold")
    svg.text(100, y + 52, "Scan 10,000 headers; 486 levels contain 1,465 heap orders", size=13, anchor="start", fill="#4B5563")
    header_x_values = (500, 740, 980)
    labels = ("level i", "level i + 2", "level i + 8")
    colors = (baseline_color, near_color, far_color)
    for x, label, color in zip(header_x_values, labels, colors):
        svg.rect(x, y + 38, 145, 38, fill=header_color, stroke=color, stroke_width=2, rx=4)
        svg.text(x + 72.5, y + 63, label, size=13, weight="bold")
        svg.line(x + 72.5, y + 76, x + 72.5, y + 101, stroke=color, stroke_width=2)
        svg.rect(x + 17, y + 101, 111, 34, fill=heap_color, stroke=color, stroke_width=2, rx=4)
        svg.text(x + 72.5, y + 123, "heap orders", size=12)
    svg.text(860, y + 151, "Hints target heap data only when the future level is non-empty.", size=12, fill="#6B7280")

    # Simulated market sweep.
    y = card_y_values[3]
    svg.text(100, y + 28, "4. Simulated market sweep", size=17, anchor="start", weight="bold")
    svg.text(100, y + 52, "Consume 3 orders at each of 20 consecutive levels", size=13, anchor="start", fill="#4B5563")
    bar_left, bar_y = 455, y + 45
    svg.rect(bar_left, bar_y, 470, 48, fill=header_color, rx=3)
    svg.rect(bar_left + 470, bar_y, 105, 48, fill="#34D399")
    svg.rect(bar_left + 575, bar_y, 120, 48, fill="#DDD6FE", rx=3)
    svg.text(bar_left + 235, bar_y + 30, "indices 0–4,999: empty scan", size=14, weight="bold", fill="#374151")
    svg.text(bar_left + 522.5, bar_y + 30, "20 levels", size=12, weight="bold")
    svg.text(bar_left + 635, bar_y + 30, "50 noise draws", size=12)
    svg.line(bar_left + 450, y + 113, bar_left + 520, y + 113, stroke=near_color, stroke_width=3)
    svg.text(bar_left + 485, y + 137, "look ahead at most four headers", size=12, fill="#C2410C")

    svg.rect(310, 812, 18, 18, fill=baseline_color, rx=2)
    svg.text(338, 826, "current access", size=12, anchor="start")
    svg.rect(485, 812, 18, 18, fill=near_color, rx=2)
    svg.text(513, 826, "near prefetch", size=12, anchor="start")
    svg.rect(670, 812, 18, 18, fill=far_color, rx=2)
    svg.text(698, 826, "far prefetch", size=12, anchor="start")
    svg.text(1010, 826, "All hints use _MM_HINT_T0.", size=12, fill="#6B7280")
    svg.write(path)


def draw_prefetch_latency_figure(
    rows: dict[tuple[str, str], dict[str, str]],
    path: Path,
    metric: str,
) -> None:
    width, height = 1500, 860
    svg = Svg(width, height)
    metric_label = "median" if metric == "p50_ns" else "p99"
    svg.text(width / 2, 40, f"Software-prefetch {metric_label} batch latency", size=25, weight="bold")
    svg.text(
        width / 2,
        66,
        "1,000 samples per bar; each panel has its own scale and batch definition",
        size=14,
        fill="#4B5563",
    )

    panel_lefts = (95, 825, 95, 825)
    panel_tops = (140, 140, 500, 500)
    panel_width, plot_height = 580, 235

    for panel_index, (operation, title, batch, variants) in enumerate(PREFETCH_OPERATIONS):
        left = panel_lefts[panel_index]
        top = panel_tops[panel_index]
        bottom = top + plot_height
        values = [float(rows[(operation, variant)][metric]) for variant, _ in variants]
        y_max = nice_max(max(values) * 1.15)

        svg.text(left + panel_width / 2, top - 43, title, size=18, weight="bold")
        svg.text(left + panel_width / 2, top - 22, f"Timed batch: {batch}", size=12, fill="#4B5563")
        for tick in range(6):
            value = y_max * tick / 5
            y = bottom - plot_height * value / y_max
            svg.line(left, y, left + panel_width, y, stroke="#D1D5DB", dash="4 4")
            svg.text(left - 10, y + 5, format_ns(value), size=11, anchor="end", fill="#4B5563")
        svg.line(left, top, left, bottom, stroke="#374151", stroke_width=1.5)
        svg.line(left, bottom, left + panel_width, bottom, stroke="#374151", stroke_width=1.5)

        bar_count = len(variants)
        spacing = panel_width / (bar_count + 1)
        for index, ((_, label), value) in enumerate(zip(variants, values)):
            center = left + spacing * (index + 1)
            bar_width = 78
            bar_height = plot_height * value / y_max
            svg.rect(
                center - bar_width / 2,
                bottom - bar_height,
                bar_width,
                bar_height,
                fill=PREFETCH_COLORS[index],
                rx=3,
            )
            svg.text(center, bottom - bar_height - 8, format_ns(value), size=11, weight="bold")
            svg.text(center, bottom + 24, label, size=12, weight="bold")

        if panel_index in (0, 2):
            svg.text(left - 68, (top + bottom) / 2, "Batch latency", size=14, rotate=-90)

    svg.text(
        width / 2,
        830,
        "Lower is better. Bars are comparable within a panel; all prefetch variants include hint-generation overhead.",
        size=13,
        fill="#4B5563",
    )
    svg.write(path)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--scenario",
        choices=tuple(SCENARIO_TITLES),
        default="uniform",
        help="Scenario to plot (default: uniform)",
    )
    parser.add_argument(
        "--input",
        type=Path,
        help=(
            "Benchmark result CSV (default: results/scenario_<scenario>.csv; "
            "optimization experiments use results/bench_<scenario>.csv)"
        ),
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path("figures"),
        help="Directory for generated SVGs (default: figures)",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    default_name = (
        f"bench_{args.scenario}.csv"
        if args.scenario in ("alignment", "prefetch")
        else f"scenario_{args.scenario}.csv"
    )
    input_path = args.input or Path("results") / default_name

    if args.scenario == "sweep":
        rows = load_sweep_rows(input_path)
        names = (
            "sweep_latency_p50.svg",
            "sweep_latency_p99.svg",
            "sweep_workload_model.svg",
        )
        draw_sweep_latency_figure(rows, args.output_dir / names[0], "p50_ns")
        draw_sweep_latency_figure(rows, args.output_dir / names[1], "p99_ns")
        draw_sweep_model(args.output_dir / names[2])
        for name in names:
            print(args.output_dir / name)
        return

    if args.scenario == "buildup":
        rows = load_buildup_rows(input_path)
        names = (
            "buildup_latency_p50.svg",
            "buildup_latency_p99.svg",
            "buildup_workload_model.svg",
        )
        draw_buildup_latency_figure(rows, args.output_dir / names[0], "p50_ns")
        draw_buildup_latency_figure(rows, args.output_dir / names[1], "p99_ns")
        draw_buildup_model(args.output_dir / names[2])
        for name in names:
            print(args.output_dir / name)
        return

    if args.scenario == "alignment":
        rows = load_alignment_rows(input_path)
        names = (
            "alignment_latency_p50.svg",
            "alignment_latency_p99.svg",
            "alignment_layout_model.svg",
        )
        draw_alignment_latency_figure(rows, args.output_dir / names[0], "p50_ns")
        draw_alignment_latency_figure(rows, args.output_dir / names[1], "p99_ns")
        draw_alignment_model(args.output_dir / names[2])
        for name in names:
            print(args.output_dir / name)
        return

    if args.scenario == "prefetch":
        rows = load_prefetch_rows(input_path)
        names = (
            "prefetch_latency_p50.svg",
            "prefetch_latency_p99.svg",
            "prefetch_workload_model.svg",
        )
        draw_prefetch_latency_figure(rows, args.output_dir / names[0], "p50_ns")
        draw_prefetch_latency_figure(rows, args.output_dir / names[1], "p99_ns")
        draw_prefetch_model(args.output_dir / names[2])
        for name in names:
            print(args.output_dir / name)
        return

    rows = load_rows(input_path)
    prefix = args.scenario
    scenario_title = SCENARIO_TITLES[args.scenario]
    names = (
        f"{prefix}_latency_p50_p99.svg",
        f"{prefix}_latency_percentiles.svg",
        f"{prefix}_workload_model.svg",
    )

    draw_bar_figure(rows, args.output_dir / names[0], scenario_title, input_path)
    draw_percentile_figure(rows, args.output_dir / names[1], scenario_title)
    if args.scenario == "uniform":
        draw_uniform_model(args.output_dir / names[2])
    elif args.scenario == "clustered":
        draw_clustered_model(args.output_dir / names[2])
    elif args.scenario == "zipfian":
        draw_zipfian_model(args.output_dir / names[2])
    elif args.scenario == "bursty":
        draw_bursty_model(args.output_dir / names[2])
    else:
        draw_high_cancel_model(args.output_dir / names[2])

    for name in names:
        print(args.output_dir / name)


if __name__ == "__main__":
    main()
