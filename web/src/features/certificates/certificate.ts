import type {Certificate} from '../../api/types';

const DAY = 86_400;

/** A wildcard covers exactly one extra label, like the server's own matching. */
export function covers(domain: string, host: string) {
  if (!domain.startsWith('*.')) return domain === host;
  const [label, ...rest] = host.split('.');
  return label !== '' && rest.join('.') === domain.slice(2);
}

/** The certificate that serves a host: the longest-lived among those covering it. */
export function certificateFor(certificates: Certificate[], host: string): Certificate | undefined {
  return certificates
    .filter((certificate) => certificate.domains.some((domain) => covers(domain, host)))
    .sort((a, b) => (b.not_after ?? 0) - (a.not_after ?? 0))[0];
}

export function daysLeft(certificate: Certificate, now = Date.now() / 1000): number | null {
  return certificate.not_after === null ? null : Math.floor((certificate.not_after - now) / DAY);
}

/** Renewal starts 30 days before expiry, so less than a few weeks left means renewal is failing. */
export const isAtRisk = (certificate: Certificate) =>
  certificate.status === 'valid' && (daysLeft(certificate) ?? Infinity) < 15;

/** How worrying a certificate's state is: healthy, in progress or degraded, or broken. */
export function tone(certificate: Certificate): 'ok' | 'warn' | 'bad' {
  if (certificate.status === 'expired' || certificate.status === 'failed') return 'bad';
  const healthy = certificate.status === 'valid' && !certificate.last_error && !isAtRisk(certificate);
  return healthy ? 'ok' : 'warn';
}

export function expiryText(certificate: Certificate): string {
  const days = daysLeft(certificate);
  if (days === null) return '—';
  if (days < 0) return 'Expired';
  if (days === 0) return 'Expires today';
  return days === 1 ? 'Expires in 1 day' : `Expires in ${days} days`;
}

export const dateText = (seconds: number) => new Date(seconds * 1000).toLocaleDateString(undefined, {dateStyle: 'medium'});

/** "in 5 minutes", for the next automatic attempt. */
export function retryText(certificate: Certificate, now = Date.now() / 1000): string {
  if (certificate.next_attempt === null) return '';
  const minutes = Math.max(1, Math.round((certificate.next_attempt - now) / 60));
  return minutes < 90 ? `Retrying in ${minutes} min` : `Retrying in ${Math.round(minutes / 60)} h`;
}

export const STATUS_LABEL: Record<Certificate['status'], string> = {
  pending: 'Waiting',
  issuing: 'Issuing…',
  valid: 'Valid',
  expired: 'Expired',
  failed: 'Failed',
};
