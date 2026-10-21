import type {Route, RouteInput} from '../../api/types';

/** Editable form state; an empty forward-auth URL means "public". */
export type HostForm = Omit<RouteInput, 'forward_auth_url'> & {forward_auth_url: string};

export const EMPTY_HOST_FORM: HostForm = {
  name: '',
  host: '',
  path_prefix: '/',
  upstream: 'http://127.0.0.1:3000',
  enabled: true,
  forward_auth_url: '',
};

export function toForm(route: Route): HostForm {
  const {id: _id, ...input} = route;
  return {...input, forward_auth_url: input.forward_auth_url ?? ''};
}

export function toInput(form: HostForm): RouteInput {
  return {...form, forward_auth_url: form.forward_auth_url.trim() || null};
}

export const isProtected = (route: Route) => Boolean(route.forward_auth_url);

export function matchesQuery(route: Route, query: string) {
  const haystack = `${route.name} ${route.host} ${route.upstream}`.toLowerCase();
  return haystack.includes(query.toLowerCase());
}
