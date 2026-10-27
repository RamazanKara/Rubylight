// USB devices the paired devices forward to this PC, as the console lists them.
import type { Client, UsbAttachment } from '../../lib/api';

/** The attachments of each paired device, devices in name order, each one's in name order. */
export function byClient(devices: UsbAttachment[]): { client: string; devices: UsbAttachment[] }[] {
  const groups = new Map<string, UsbAttachment[]>();
  for (const device of devices) groups.set(device.client, [...(groups.get(device.client) ?? []), device]);
  return [...groups]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([client, list]) => ({ client, devices: list.sort((a, b) => a.name.localeCompare(b.name) || a.id - b.id) }));
}

const hex = (n: number) => n.toString(16).padStart(4, '0');

/** `046d:c52b`, as `lsusb` and Device Manager spell it. */
export function usbId(device: Pick<UsbAttachment, 'vid' | 'pid'>): string {
  return `${hex(device.vid)}:${hex(device.pid)}`;
}

export function usbState(device: Pick<UsbAttachment, 'state' | 'port'>): { label: string; tone: 'neutral' | 'ok' | 'warn' | 'danger' } {
  switch (device.state) {
    case 'waiting':
      return { label: 'Waiting for the device', tone: 'neutral' };
    case 'opening':
      return { label: 'Connecting', tone: 'neutral' };
    case 'attached':
      return { label: device.port === null ? 'Attached' : `Attached on port ${device.port}`, tone: 'ok' };
    case 'detaching':
      return { label: 'Detaching', tone: 'warn' };
    case 'detach_failed':
      return { label: 'Detach failed', tone: 'danger' };
    default:
      // A state a newer host adds is shown as it is, never hidden.
      return { label: String(device.state), tone: 'neutral' };
  }
}

/** The panel shows once a device may forward or something is forwarded. */
export function showUsb(clients: Client[] | null, devices: UsbAttachment[]): boolean {
  return devices.length > 0 || (clients ?? []).some((client) => client.allow_usb === true);
}
