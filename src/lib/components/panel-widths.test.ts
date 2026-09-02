import { describe, expect, it } from "vitest";
import {
  defaultPanelWidth,
  fitPanelWidths,
  fitPluginSideWidth,
  resizePanelWidth,
} from "./panel-widths.ts";

const base = {
  containerWidth: 1200,
  mainMinWidth: 320,
  panelMinWidth: 280,
  defaultWidth: 380,
  aiVisible: true,
  sftpVisible: true,
  aiWidth: null,
  sftpWidth: null,
};

describe("fitPanelWidths", () => {
  it("clamps two restored widths without changing their stored preferences", () => {
    const fitted = fitPanelWidths({ ...base, aiWidth: 800, sftpWidth: 800 });

    expect(fitted).toEqual({ ai: 440, sftp: 440 });
    expect(fitted.ai + fitted.sftp + base.mainMinWidth).toBe(base.containerWidth);
  });

  it("gives unused space from the smaller panel to the larger panel", () => {
    expect(fitPanelWidths({ ...base, aiWidth: 280, sftpWidth: 800 })).toEqual({
      ai: 280,
      sftp: 600,
    });
  });

  it("keeps the dragged panel stable when its fitted width becomes its preference", () => {
    const initial = fitPanelWidths({ ...base, aiWidth: 800, sftpWidth: 800 });
    const afterDrag = fitPanelWidths({
      ...base,
      aiWidth: initial.ai,
      sftpWidth: 800,
    });

    expect(afterDrag).toEqual(initial);
  });

  it("re-clamps on a narrow container instead of overflowing", () => {
    const fitted = fitPanelWidths({
      ...base,
      containerWidth: 700,
      aiWidth: 800,
      sftpWidth: 800,
    });

    expect(fitted).toEqual({ ai: 190, sftp: 190 });
    expect(fitted.ai + fitted.sftp + base.mainMinWidth).toBe(700);
  });

  it("does not reserve width for a hidden opposite panel", () => {
    expect(fitPanelWidths({
      ...base,
      sftpVisible: false,
      aiWidth: 800,
      sftpWidth: 800,
    })).toEqual({ ai: 800, sftp: 800 });
  });

  it("preserves the responsive default for one panel on a narrow window", () => {
    expect(fitPanelWidths({
      ...base,
      containerWidth: 500,
      defaultWidth: 320,
      sftpVisible: false,
    })).toEqual({ ai: 320, sftp: 320 });
  });

  it("keeps one restored panel usable when the main minimum cannot also fit", () => {
    expect(fitPanelWidths({
      ...base,
      containerWidth: 500,
      defaultWidth: 320,
      aiWidth: 800,
      sftpVisible: false,
    })).toEqual({ ai: 280, sftp: 320 });
  });

  it("uses the viewport breakpoint rather than the sidebar-reduced content width", () => {
    expect(defaultPanelWidth(850)).toBe(380);
    expect(defaultPanelWidth(800)).toBe(320);
  });

  it("lets one gesture shrink and then restore against its captured opposite width", () => {
    const gesture = {
      startWidth: 440,
      sign: 1,
      minWidth: 280,
      containerWidth: 1200,
      mainMinWidth: 320,
      otherPanelVisible: true,
    };

    expect(resizePanelWidth({ ...gesture, deltaX: -10 })).toBe(430);
    expect(resizePanelWidth({ ...gesture, deltaX: 0 })).toBe(440);
  });

  it("changes only the gesture-priority preference and restores the opposite preference later", () => {
    const sftpPreference = 800;
    const resizedAi = resizePanelWidth({
      startWidth: 440,
      deltaX: -10,
      sign: 1,
      minWidth: 280,
      containerWidth: 1200,
      mainMinWidth: 320,
      otherPanelVisible: true,
    });

    expect(resizedAi).toBe(430);
    expect(fitPanelWidths({
      ...base,
      aiWidth: resizedAi,
      sftpWidth: sftpPreference,
    })).toEqual({ ai: 430, sftp: 450 });
    expect(fitPanelWidths({
      ...base,
      aiVisible: false,
      aiWidth: resizedAi,
      sftpWidth: sftpPreference,
    })).toEqual({ ai: 430, sftp: sftpPreference });
  });

  it("lets either fitted panel grow without overwriting the opposite preference", () => {
    const initial = fitPanelWidths({
      ...base,
      aiWidth: 500,
      sftpWidth: 500,
    });
    expect(initial).toEqual({ ai: 440, sftp: 440 });

    const aiPreference = resizePanelWidth({
      startWidth: initial.ai,
      deltaX: 40,
      sign: 1,
      minWidth: base.panelMinWidth,
      containerWidth: base.containerWidth,
      mainMinWidth: base.mainMinWidth,
      otherPanelVisible: true,
    });
    expect(aiPreference).toBe(480);
    const afterAiDrag = fitPanelWidths({
      ...base,
      aiWidth: aiPreference,
      sftpWidth: 500,
      priority: "ai",
    });
    expect(afterAiDrag).toEqual({ ai: 480, sftp: 400 });

    const sftpPreference = resizePanelWidth({
      startWidth: afterAiDrag.sftp,
      deltaX: 80,
      sign: 1,
      minWidth: base.panelMinWidth,
      containerWidth: base.containerWidth,
      mainMinWidth: base.mainMinWidth,
      otherPanelVisible: true,
    });
    expect(sftpPreference).toBe(480);
    expect(fitPanelWidths({
      ...base,
      aiWidth: aiPreference,
      sftpWidth: sftpPreference,
      priority: "sftp",
    })).toEqual({ ai: 400, sftp: 480 });
  });

  it("keeps the main pane and opposite panel minima under drag priority", () => {
    expect(fitPanelWidths({
      ...base,
      aiWidth: 700,
      sftpWidth: 500,
      priority: "ai",
    })).toEqual({ ai: 600, sftp: 280 });
  });
});

describe("fitPluginSideWidth", () => {
  const pbase = {
    containerWidth: 1200,
    mainMinWidth: 320,
    panelMinWidth: 280,
    defaultWidth: 380,
    aiVisible: true,
    sftpVisible: true,
  };

  it("hidden plugin keeps the full container for ai/sftp", () => {
    const fit = fitPluginSideWidth({...pbase, pluginVisible: false, pluginWidth: null});
    expect(fit.plugin).toBe(380);
    expect(fit.remainingContainerWidth).toBe(1200);
  });

  it("visible plugin takes its preference when space is plentiful", () => {
    // 1400 - 320(main) - 2×280(other minima) = 520 ≥ 340 → preference wins.
    const fit = fitPluginSideWidth({...pbase, containerWidth: 1400, pluginVisible: true, pluginWidth: 340});
    expect(fit.plugin).toBe(340);
    expect(fit.remainingContainerWidth).toBe(1060);
  });

  it("yields toward the minimum before the other panels' minima", () => {
    // 1000 - 320(main) = 680 budget; ai+sftp reserve 2×280 → plugin max 120
    // < min 280 → plugin clamps to its min and ai/sftp negotiate the rest.
    const fit = fitPluginSideWidth({
      ...pbase, containerWidth: 1000, pluginVisible: true, pluginWidth: 600,
    });
    expect(fit.plugin).toBe(280);
    expect(fit.remainingContainerWidth).toBe(720);
  });

  it("plugin min never pushes the remainder below the main minimum entirely", () => {
    // Extreme narrow: plugin still renders at min; remainder may be tight and
    // fitPanelWidths degrades gracefully from there.
    const fit = fitPluginSideWidth({
      ...pbase, containerWidth: 500, pluginVisible: true, pluginWidth: null,
    });
    expect(fit.plugin).toBeGreaterThanOrEqual(280);
    expect(fit.remainingContainerWidth).toBe(500 - fit.plugin);
  });

  it("a null preference uses the responsive default", () => {
    const fit = fitPluginSideWidth({
      ...pbase, containerWidth: 1400, pluginVisible: true, pluginWidth: null,
    });
    expect(fit.plugin).toBe(380);
  });

  it("chaining into fitPanelWidths keeps ai/sftp inside the remainder", () => {
    const {plugin, remainingContainerWidth} = fitPluginSideWidth({
      ...pbase, containerWidth: 1100, pluginVisible: true, pluginWidth: 400,
    });
    const two = fitPanelWidths({
      containerWidth: remainingContainerWidth,
      mainMinWidth: 320,
      panelMinWidth: 280,
      defaultWidth: 380,
      aiVisible: true,
      sftpVisible: true,
      aiWidth: 500,
      sftpWidth: 500,
    });
    expect(plugin + two.ai + two.sftp).toBeLessThanOrEqual(1100 - 320 + 280 * 2 - 320 + 320);
  });
});
