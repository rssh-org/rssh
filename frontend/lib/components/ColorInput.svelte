<script lang="ts">
  import { untrack } from "svelte";
  import * as colorPicker from "@zag-js/color-picker";
  import { normalizeProps, useMachine } from "@zag-js/svelte";
  import { t } from "../i18n/index.svelte.ts";
  import AppIcon from "./AppIcon.svelte";
  import { isColorInputValid, normalizeHexColor } from "./color-input.ts";

  let { value = $bindable(), savedValue }: { value: string; savedValue?: string } = $props();
  const id = $props.id();
  const presets = [
    "#4A6CF7", "#FF6B6B", "#FFB86C", "#50FA7B",
    "#8BE9FD", "#BD93F9", "#000000", "#FFFFFF",
  ];
  const normalized = $derived(normalizeHexColor(value));
  const valid = $derived(isColorInputValid(value, savedValue));
  let pickerColor = $state(untrack(() => parsePickerColor(value)));

  const service = useMachine(colorPicker.machine, () => ({
    id,
    inline: true,
    defaultFormat: "hsba",
    value: pickerColor,
    invalid: !valid,
    onValueChange({ value: next }) {
      pickerColor = next;
      value = next.toString("hex").toUpperCase();
    },
  }));
  const picker = $derived(colorPicker.connect(service, normalizeProps));
  const hueThumbProps = $derived(picker.getChannelSliderThumbProps({ channel: "hue" }));
  const saturation = $derived(Math.round(pickerColor.getChannelValue("saturation")));
  const brightness = $derived(Math.round(pickerColor.getChannelValue("brightness")));
  const hue = $derived(Math.round(pickerColor.getChannelValue("hue")));

  function parsePickerColor(color: string) {
    try {
      return colorPicker.parse(normalizeHexColor(color) ?? color).toFormat("hsba").withChannelValue("alpha", 1);
    } catch {
      // Unrecognized legacy colors remain in the parent draft until explicitly replaced.
      return colorPicker.parse("#000000").toFormat("hsba");
    }
  }

  $effect(() => {
    if (!valid) return;
    const next = parsePickerColor(value);
    // Keep the exact HSBA state when RGB bytes agree, including hue at gray/black.
    // Synchronizing external drafts must not normalize partially typed HEX in place.
    if (next.toString("hex") !== untrack(() => pickerColor.toString("hex"))) pickerColor = next;
  });

  function normalizeDraft() {
    if (normalized) value = normalized;
  }

  function stopTouchGesture(event: TouchEvent) {
    // AppShell owns edge swipes; dragging a color must stay inside this control.
    event.stopPropagation();
  }

  function handleHueKeydown(event: KeyboardEvent) {
    // Zag 1.47.0 omits the step for these two keys, resetting hue to zero.
    if (!event.defaultPrevented && (event.key === "PageUp" || event.key === "PageDown")) {
      event.preventDefault();
      picker.setChannelValue("hue", hue + (event.key === "PageUp" ? 10 : -10));
      return;
    }
    hueThumbProps.onkeydown?.(event);
  }
</script>

<div {...picker.getRootProps()} class="color-field">
  <span class="field-label">{t("common.color")}</span>
  <details>
    <summary title={t("color_input.choose")} aria-label={`${t("color_input.choose")}: ${value}`}
      aria-describedby={valid ? undefined : `${id}-error`}>
      <span class="summary-content">
        <span class="swatch" style:background-color={normalized ?? (valid ? value : "var(--surface)")} aria-hidden="true"></span>
        <span class="color-value">{value || "—"}</span>
      </span>
    </summary>
    <div {...picker.getContentProps()} class="picker">
      <div {...picker.getAreaProps()} class="color-area" aria-label={t("color_input.area")}
        ontouchstart={stopTouchGesture} ontouchend={stopTouchGesture}>
        <div {...picker.getAreaBackgroundProps()} class="area-background"></div>
        <div {...picker.getAreaThumbProps()} class="color-thumb"
          aria-label={t("color_input.area")}
          aria-roledescription={t("color_input.two_dimensional_slider")}
          aria-valuetext={t("color_input.area_value", { saturation, brightness })}></div>
      </div>
      <div class="hue-field">
        <span class="field-label">{t("color_input.hue")}</span>
        <div {...picker.getChannelSliderProps({ channel: "hue" })} class="hue-slider"
          ontouchstart={stopTouchGesture} ontouchend={stopTouchGesture}>
          <div {...picker.getChannelSliderTrackProps({ channel: "hue" })} class="hue-track">
            <div {...hueThumbProps} class="color-thumb hue-thumb" onkeydown={handleHueKeydown}
              aria-label={t("color_input.hue")}
              aria-valuetext={t("color_input.hue_value", { hue })}></div>
          </div>
        </div>
      </div>
      <label class="hex-field" for={`${id}-hex`}>
        <span>{t("color_input.hex")}</span>
        <input id={`${id}-hex`} type="text" bind:value autocomplete="off" spellcheck={false}
          autocapitalize="characters" placeholder="#RRGGBB" aria-invalid={!valid}
          aria-describedby={valid ? undefined : `${id}-error`}
          onblur={normalizeDraft}
          onkeydown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              normalizeDraft();
            }
          }} />
      </label>
      <div class="presets" role="group" aria-label={t("color_input.presets")}>
        {#each presets as color}
          <button type="button" class="preset" style:background={color}
            title={color} aria-label={color} aria-pressed={normalized === color}
            onclick={() => value = color}>
            {#if normalized === color}
              <span class="selected"><AppIcon name="check" size={14} /></span>
            {/if}
          </button>
        {/each}
      </div>
    </div>
  </details>
  {#if !valid}
    <p id={`${id}-error`} class="error">{t("color_input.invalid_hex")}</p>
  {/if}
</div>

<style>
  .color-field { width: 280px; max-width: 100%; min-width: 0; }
  .field-label { display: block; margin-bottom: 4px; color: var(--text-sub); font-size: 12px; }
  details { border: 1px solid var(--divider); border-radius: var(--radius-sm); background: var(--surface); }
  summary { padding: 8px 12px; min-height: 44px; cursor: pointer; color: var(--text); border-radius: var(--radius-sm); }
  .summary-content { display: inline-flex; align-items: center; gap: 8px; vertical-align: middle; max-width: calc(100% - 24px); }
  .swatch { width: 24px; height: 24px; flex-shrink: 0; border: 1px solid var(--divider); border-radius: 4px; }
  .color-value { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; }
  .picker { display: flex; flex-direction: column; gap: 12px; padding: 12px; border-top: 1px solid var(--divider); }
  .color-area { height: 160px; border-radius: var(--radius-sm); }
  .area-background { width: 100%; height: 100%; border-radius: inherit; }
  .color-thumb { width: 22px; height: 22px; border: 2px solid var(--white); border-radius: 50%; box-shadow: 0 0 0 1px color-mix(in srgb, var(--black) 53%, transparent); }
  .hue-field .field-label { margin-bottom: 0; }
  .hue-slider { display: flex; align-items: center; height: 44px; padding-inline: 11px; }
  .hue-track { width: 100%; height: 16px; border-radius: 8px; }
  .hue-thumb { transform: translate(-50%, -50%); }
  .presets { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 8px; }
  .preset { display: flex; align-items: center; justify-content: center; min-height: 44px; padding: 0; border: 1px solid var(--divider); border-radius: var(--radius-sm); cursor: pointer; }
  .selected { display: flex; padding: 2px; border-radius: 50%; background: var(--surface); color: var(--text); }
  summary:focus-visible, .preset:focus-visible, .color-thumb:focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; }
  .hex-field { display: flex; flex-direction: column; gap: 4px; }
  .hex-field input { width: 100%; min-width: 0; }
  .error { margin: 4px 0 0; color: var(--error); font-size: 12px; }
</style>
