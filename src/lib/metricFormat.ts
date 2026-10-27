export type MetricNumberKind = 'percent' | 'dollars' | 'count';
export type MetricNumberStyle = 'tray' | 'row' | 'full';

const compactFormatter = new Intl.NumberFormat('en-US', {
  notation: 'compact',
  maximumFractionDigits: 1,
});
const rowNumberFormatter = new Intl.NumberFormat('en-US', {
  minimumFractionDigits: 0,
  maximumFractionDigits: 1,
});
const fullNumberFormatter = new Intl.NumberFormat('en-US', {
  minimumFractionDigits: 0,
  maximumFractionDigits: 1,
});
const currencyFormatter = new Intl.NumberFormat('en-US', {
  style: 'currency',
  currency: 'USD',
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});
const wholeDollarFormatter = new Intl.NumberFormat('en-US', {
  style: 'currency',
  currency: 'USD',
  minimumFractionDigits: 0,
  maximumFractionDigits: 0,
});

export function formatMetricNumber(
  value: number,
  kind: MetricNumberKind,
  style: MetricNumberStyle,
) {
  if (!Number.isFinite(value)) return '—';
  if (kind === 'percent') return `${Math.round(Math.min(100, Math.max(0, value)))}%`;
  if (kind === 'dollars') {
    if (Math.abs(value) >= 1000 && style !== 'full') {
      return `$${compactFormatter.format(value)}`;
    }
    return style === 'tray' ? wholeDollarFormatter.format(value) : currencyFormatter.format(value);
  }
  if (style !== 'full' && Math.abs(value) >= 1000) return compactFormatter.format(value);
  return (style === 'full' ? fullNumberFormatter : rowNumberFormatter).format(value);
}

export function formatMetricValue(
  value: number,
  kind: MetricNumberKind,
  style: MetricNumberStyle,
  label?: string,
) {
  const formatted = formatMetricNumber(value, kind, style);
  return label ? `${formatted} ${label}` : formatted;
}
