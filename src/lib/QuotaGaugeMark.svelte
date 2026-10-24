<script lang="ts">
  import quotaGaugeMark from '../assets/quotagauge-mark.png';

  interface Props {
    size?: number;
    decorative?: boolean;
  }

  let { size = 18, decorative = true }: Props = $props();
</script>

<img
  src={quotaGaugeMark}
  width={size}
  height={size}
  alt={decorative ? '' : 'QuotaGauge'}
  aria-hidden={decorative}
  draggable="false"
/>

<style>
  img {
    display: block;
    object-fit: contain;
  }
</style>
