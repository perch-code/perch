# Fixture: tour with stop verdicts

Tests that each tour stop has a required `verdict` field. It is "read", "skim", or "skip".

## Validation rules

- The schema version must be 4.
- tour.chapters[].stops[].verdict must be one of: "read", "skim", or "skip". It is always present.
- tour.chapters[].lines must count read vs skim lines based on stop verdicts.
  - A stop verdict overrides the part verdict.
  - Read verdict stops add to the .read count.
  - Skim verdict stops add to the .skim count.
