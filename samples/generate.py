"""Generate samples/sales.xlsx used for manual testing / examples."""

from pathlib import Path

from openpyxl import Workbook

rows = [
    ["Region", "Product", "Rep", "Amount", "Units"],
    ["East", "Widget", "Alice", 100, 2],
    ["East", "Widget", "Bob", 250, 5],
    ["East", "Gadget", "Alice", 300, 3],
    ["West", "Widget", "Carol", 400, 8],
    ["West", "Gadget", "Carol", 150, 1],
    ["West", "Gadget", "Dave", 150, 1],
    ["North", "Widget", "Erin", 75, 1],
]

wb = Workbook()
ws = wb.active
ws.title = "data"
for row in rows:
    ws.append(row)

out = Path(__file__).resolve().parent.parent / "samples" / "sales.xlsx"
out.parent.mkdir(parents=True, exist_ok=True)
wb.save(out)
print(f"wrote {out}")
