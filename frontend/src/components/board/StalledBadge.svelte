<script lang="ts">
  import { formatStalled } from '../../lib/pipeline';

  interface Props {
    hours: number;
  }
  let { hours }: Props = $props();
</script>

<span class="badge-stall" title="pending 超过 pending_timeout_hours（决策 34）">
  {formatStalled(hours)}
</span>

<style>
  .badge-stall {
    display: inline-block;
    padding: 1px 6px;
    border-radius: var(--r-pill);
    background: rgba(242, 179, 61, 0.12);
    color: var(--signal-caution);
    font-family: var(--font-cond);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.02em;
    white-space: nowrap;
  }
</style>
