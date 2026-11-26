/**
 * The console's display map: where each display sits on the Windows desktop, drawn to scale.
 *
 * Pure geometry over what `/api/display-devices` reports. The map is read-only: Windows arranges
 * displays, and Rubylight puts the saved layout back after a stream (Troubleshoot, Displays).
 */
import type { DisplayDevice } from '../../lib/api';

export interface Tile {
  id: string;
  /** Its number on the map and in the list, from 1. */
  index: number;
  name: string;
  /** Place and size on the map, in percent of the map. */
  left: number;
  top: number;
  width: number;
  height: number;
  /** "3840 × 2160 · 120 Hz", or empty when Windows did not report the mode. */
  mode: string;
  /** "150%", or empty. */
  scale: string;
  primary: boolean;
  /** `null` when the display has no HDR. */
  hdr: 'on' | 'off' | null;
  virtual: boolean;
  /** The display a stream is set to use. */
  target: boolean;
  /** Where it sits: "Primary display" or "Right of Studio Display". */
  place: string;
}

export interface Topology {
  /** False when Windows did not report every display's place: the tiles then stand in a row, in order. */
  spatial: boolean;
  /** Width over height of the map. */
  ratio: number;
  tiles: Tile[];
}

type Box = { x: number; y: number; width: number; height: number };

const finite = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value);

function boxOf(display: DisplayDevice): Box | null {
  const { x, y, width, height } = display;
  return finite(x) && finite(y) && finite(width) && finite(height) && width > 0 && height > 0 ? { x, y, width, height } : null;
}

/** "3840 × 2160 · 120 Hz"; the refresh is left out when Windows reports none. */
export function modeText(display: Pick<DisplayDevice, 'width' | 'height' | 'refresh'>): string {
  if (!finite(display.width) || !finite(display.height) || display.width <= 0 || display.height <= 0) return '';
  const size = `${display.width} × ${display.height}`;
  return finite(display.refresh) && display.refresh > 1 ? `${size} · ${display.refresh} Hz` : size;
}

/** Whether the saved name or id of a stream's display means this display, as the host matches it. */
export function isTarget(display: DisplayDevice, target: string | undefined): boolean {
  const wanted = target?.trim().toLowerCase();
  if (!wanted) return false;
  return [display.device_id, display.display_name].some((value) => value?.toLowerCase() === wanted);
}

/** Where `box` sits from `anchor`, by the larger distance between their centres. */
export function sideOf(box: Box, anchor: Box): 'Left of' | 'Right of' | 'Above' | 'Below' | 'On top of' {
  const dx = box.x + box.width / 2 - (anchor.x + anchor.width / 2);
  const dy = box.y + box.height / 2 - (anchor.y + anchor.height / 2);
  if (dx === 0 && dy === 0) return 'On top of';
  if (Math.abs(dx) >= Math.abs(dy)) return dx < 0 ? 'Left of' : 'Right of';
  return dy < 0 ? 'Above' : 'Below';
}

/** The map of the displays the host lists; `target` is the display a stream is set to use. */
export function topology(displays: readonly DisplayDevice[], target?: string): Topology {
  const boxes = displays.map(boxOf);
  const spatial = displays.length > 0 && boxes.every((box) => box !== null);
  // Without every place, equal tiles in the order Windows lists them.
  const placed: Box[] = spatial
    ? (boxes as Box[])
    : displays.map((_display, index) => ({ x: index * 1920, y: 0, width: 1920, height: 1080 }));
  const minX = placed.length ? Math.min(...placed.map((box) => box.x)) : 0;
  const minY = placed.length ? Math.min(...placed.map((box) => box.y)) : 0;
  const maxX = Math.max(1, ...placed.map((box) => box.x + box.width - minX));
  const maxY = Math.max(1, ...placed.map((box) => box.y + box.height - minY));
  const names = displays.map((display, index) => display.friendly_name?.trim() || `Display ${index + 1}`);
  const anchorIndex = Math.max(0, displays.findIndex((display) => display.primary));
  const anchor = placed[anchorIndex];
  const tiles = displays.map((display, index): Tile => {
    const box = placed[index] as Box;
    const first = index === anchorIndex;
    let place = display.primary ? 'Primary display' : '';
    if (!first && anchor) place = spatial ? `${sideOf(box, anchor)} ${names[anchorIndex]}` : '';
    return {
      id: display.device_id || display.display_name || String(index),
      index: index + 1,
      name: names[index] as string,
      left: ((box.x - minX) / maxX) * 100,
      top: ((box.y - minY) / maxY) * 100,
      width: (box.width / maxX) * 100,
      height: (box.height / maxY) * 100,
      mode: modeText(display),
      scale: finite(display.scale) && display.scale > 0 ? `${display.scale}%` : '',
      primary: display.primary,
      hdr: display.hdr_supported ? (display.hdr_enabled ? 'on' : 'off') : null,
      virtual: display.virtual === true,
      target: isTarget(display, target),
      place,
    };
  });
  return { spatial, ratio: maxX / maxY, tiles };
}

/** One sentence for a display, as a screen reader hears it. */
export function describe(tile: Tile): string {
  return [
    tile.name,
    tile.mode.replace(' · ', ' at '),
    tile.scale && `${tile.scale} scale`,
    tile.hdr && `HDR ${tile.hdr}`,
    tile.virtual ? 'virtual display' : '',
    tile.place && tile.place.toLowerCase(),
    tile.target ? 'streams to this device' : '',
  ]
    .filter(Boolean)
    .join(', ');
}
