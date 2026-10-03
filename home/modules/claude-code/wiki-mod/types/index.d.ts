export type WikiPage = {
  path: string
  rel: string
  title: string
  type: string
}

export type WikiSection = { heading: string; start: number; end: number }
export type WikiRelatedPage = WikiPage & { section: WikiSection | null }
export type WikiSuggestion = WikiPage & { sources: string[] }
export type WikiHit = WikiPage & { updated: string }
export type WikiStats = {
  suggested: WikiSuggestion[]
  subagents: number
  curator: string | null
  refreshId: number
}
export type WikiPane = {
  query: string
  hits: WikiHit[]
  searching: boolean
  searched: boolean
  searchId: number
  searchError: string
  selected: WikiPage | null
  preview: string
  loading: boolean
  selectionId: number
  previewError: string
  actionError: string
}

export type WikiReviewItem = {
  id: string
  kind: 'branch' | 'diff'
  branch: string | null
  diff: string | null
  commit: string | null
  type: string
  pages: string[]
  outcome: 'rejected' | 'deferred'
  reason: string
  summary: string
  problems: string[]
}
export type WikiReviewAction = 'land' | 'discard'
export type WikiReviewConfirmation = {
  id: string
  action: WikiReviewAction
  revision: string
  token: number
}
export type WikiReview = {
  items: WikiReviewItem[]
  listRequest: number
  listLoading: boolean
  listError: string
  selectedId: string | null
  selectionRequest: number
  diff: string
  diffLoading: boolean
  diffTruncated: boolean
  diffError: string
  confirmation: WikiReviewConfirmation | null
  actionToken: number
  operation: WikiReviewConfirmation | null
  message: string
}

declare module 'claude-code' {
  interface PluginState {
    'wiki-mod': { stats: WikiStats; pane: WikiPane; review: WikiReview }
  }
}
