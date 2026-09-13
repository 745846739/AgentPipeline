<script lang="ts">
  import { formatStalled } from '../../lib/pipeline';

  interface Props {
    hours: number;
  }
  let { hours }: Props = $props();
</script>

<span class="stalltag" title="pending 超过 pending_timeout_hours（决策 34）">
  {formatStalled(hours)}
</span>

<style>
  .stalltag {
    display: inline-block;
    flex: none;
    color: var(--pending);
    font-size: 12px;
    letter-spacing: 0.04em;
    white-space: nowrap;
  }
</style>
