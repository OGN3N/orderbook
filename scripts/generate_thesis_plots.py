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
}


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
        help="Benchmark result CSV (default: results/scenario_<scenario>.csv)",
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
    input_path = args.input or Path(f"results/scenario_{args.scenario}.csv")
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
    else:
        draw_zipfian_model(args.output_dir / names[2])

    for name in names:
        print(args.output_dir / name)


if __name__ == "__main__":
    main()
