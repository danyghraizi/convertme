import { For, Show } from 'solid-js';
import { categoryLabel } from '../catalog';
import type { Segment } from '../types';

interface ToolInfoProps {
  tool: Segment;
  category: string;
}

/**
 * The rail beside the settings sheet: what the tool does, in the tool's own words.
 *
 * All of it is product copy from the tool table (`title`, `blurb`, `benefits`,
 * `eyebrow`); nothing here is composed on the fly, so the wheel, the rail and the
 * dialog's primary button can never disagree about what a tool is called.
 */
export default function ToolInfo(props: ToolInfoProps) {
  return (
    <aside class="dialog-rail" aria-label="About this tool">
      <p class="rail-eyebrow">{props.tool.eyebrow ?? categoryLabel(props.category)}</p>
      <p class="rail-title" id="rail-title">
        {props.tool.title ?? props.tool.label}
      </p>
      <Show when={props.tool.blurb}>
        {(blurb) => <p class="rail-copy">{blurb()}</p>}
      </Show>
      <Show when={(props.tool.benefits?.length ?? 0) > 0}>
        <hr class="rail-rule" />
        <ul class="rail-benefits">
          <For each={props.tool.benefits}>
            {(benefit) => (
              <li>
                <svg class="rail-check" viewBox="0 0 18 18" aria-hidden="true" fill="none">
                  <circle cx="9" cy="9" r="8" stroke="currentColor" stroke-opacity="0.35" />
                  <path
                    d="M5.4 9.3l2.3 2.3 4.6-4.9"
                    stroke="currentColor"
                    stroke-width="1.6"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  />
                </svg>
                <span>{benefit}</span>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </aside>
  );
}
