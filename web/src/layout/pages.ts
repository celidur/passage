export type Page = 'hosts' | 'certificates';

export const PAGE_TITLES: Record<Page, string> = {
  hosts: 'Proxy hosts',
  certificates: 'Certificates',
};
