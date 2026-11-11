const $ = id => document.getElementById(id)
const native = window.__TAURI__.core
let state = { configured: false, task: { running: false } }
let folders = { inputs: '', gallery: '', exports: '' }
let cancelling = false
let currentView = 'settings'
let galleryView = 'browse'
let busy = false
let polling = false
let page = 1
let pages = 1
let browsePage = 1
let browsePages = 1
let trashPage = 1
let trashPages = 1
let browseLoaded = false
let galleryOptionsLoaded = false
let trashLoaded = false
const titles = {
  inputs: ['Inputs e importazione', 'Scegli gli archivi da aggiungere alla tua gallery.'],
  gallery: ['Gallery e date', 'Sfoglia i tuoi ricordi e scegli cosa conservare.'],
  cleanup: ['Pulizia assistita', 'Rivedi file grandi, scatti ravvicinati e suggerimenti visivi locali.'],
  exports: ['Esportazione ZIP', 'Prepara una copia annuale del tuo archivio.'],
  tasks: ['Attività', 'Segui le operazioni in corso e consulta il risultato.'],
  settings: ['Impostazioni', 'Scegli dove conservare i tuoi archivi.']
}
function showError(error) {
  $('error').textContent = error?.message || String(error)
  $('error').hidden = false
  $('error-box').hidden = false
}
function clearError() { $('error').hidden = true; $('error-box').hidden = true }
function view(name) {
  if (!state.configured && name !== 'settings') name = 'settings'
  currentView = name
  for (const key of Object.keys(titles)) $(key).hidden = key !== name
  document.querySelectorAll('[data-view]').forEach(button => {
    const active = button.dataset.view === name
    button.classList.toggle('active', active)
    if (active) button.setAttribute('aria-current', 'page')
    else button.removeAttribute('aria-current')
  })
  $('title').textContent = titles[name][0]
  $('subtitle').textContent = titles[name][1]
  if (name === 'gallery' && state.configured && !state.task.running) run(loadActiveGallery, true)
  if (name === 'cleanup' && state.configured && !state.task.running && !cleanupLoaded) run(() => loadCleanup(cleanupPage), true)
}
async function api(path, body) {
  if (!state.configured && !['/api/state', '/api/settings'].includes(path)) {
    throw new Error('Scegli e salva le tre cartelle nelle impostazioni prima di iniziare.')
  }
  const result = await native.invoke('api_request', {
    method: body === undefined ? 'GET' : 'POST', path, body: body === undefined ? null : body
  })
  if (result?.error) {
    if (body === undefined && ['/api/state', '/api/settings'].includes(path)) showError(new Error(result.error))
    else throw new Error(result.error)
  }
  return result
}
function bytes(value) {
  if (value < 1024) return `${value} B`
  if (value < 1024 ** 2) return `${(value / 1024).toFixed(1)} KB`
  if (value < 1024 ** 3) return `${(value / 1024 ** 2).toFixed(1)} MB`
  return `${(value / 1024 ** 3).toFixed(2)} GB`
}
function element(tag, className, text) {
  const node = document.createElement(tag)
  if (className) node.className = className
  if (text !== undefined) {
    node.textContent = text
    if (className?.split(' ').includes('path') || className === 'file-name') node.title = text
  }
  return node
}
function actionButton(label, operation, className = '') {
  const button = element('button', className, label)
  button.dataset.mutation = ''
  if (label.includes('cestino')) button.title = 'Il file resta sul disco e può essere ripristinato dalla Gallery.'
  button.addEventListener('click', () => run(operation, true))
  return button
}
function emptyState(title, description) {
  const root = element('div', 'empty')
  root.append(element('strong', '', title), element('p', '', description))
  return root
}
async function withLoading(id, operation) {
  const root = $(id)
  root.setAttribute('aria-busy', 'true')
  root.classList.toggle('is-loading', true)
  const hadContent = root.children.length > 0
  if (!hadContent) root.append(emptyState('Caricamento…', 'Preparazione dei file da visualizzare.'))
  try { return await operation() } catch (error) {
    const countId = { 'gallery-items': 'browse-count', conflicts: 'conflict-count' }[id]
    if (countId) $(countId).textContent = 'Caricamento non riuscito. Aggiorna per riprovare.'
    if (!hadContent) root.replaceChildren(emptyState('Caricamento non riuscito', 'Controlla il messaggio di errore e aggiorna la vista per riprovare.'))
    throw error
  } finally {
    root.setAttribute('aria-busy', 'false')
    root.classList.toggle('is-loading', false)
  }
}
function updateInputSelection() {
  const selected = $('input-list').querySelectorAll('input:checked').length
  const total = (state.inputs || []).length
  $('input-summary').textContent = total ? `${selected} di ${total} ZIP selezionati` : 'Aggiungi i tuoi archivi per iniziare.'
  const locked = busy || state.task.running || !state.configured
  $('select-inputs').disabled = locked || !total || selected === total
  $('clear-inputs').disabled = locked || !selected
  $('import').disabled = locked || !selected
}
function selectInputs(checked) {
  if (busy || state.task.running || !state.configured) return
  $('input-list').querySelectorAll('input').forEach(input => { input.checked = checked })
  updateInputSelection()
}
function enableTabKeyboard(selector) {
  const tabs = [...document.querySelectorAll(selector)]
  tabs.forEach((tab, index) => tab.addEventListener('keydown', event => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
    const direction = event.key === 'ArrowLeft' ? -1 : 1
    let target = event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : (index + direction + tabs.length) % tabs.length
    if (tabs[target].disabled) return
    event.preventDefault()
    tabs[target].focus()
    tabs[target].click()
  }))
}
function renderFiles(id, files, selectable) {
  const root = $(id)
  const signature = JSON.stringify(files)
  if (root.dataset.signature === signature) return
  root.dataset.signature = signature
  const selected = new Set([...root.querySelectorAll('input:checked')].map(input => input.value))
  root.replaceChildren()
  if (!files.length) root.append(emptyState(
    selectable ? 'Nessuno ZIP da importare' : 'Nessuna esportazione disponibile',
    selectable ? 'Aggiungi gli ZIP nella cartella Inputs, poi aggiorna l’elenco.' : 'Crea gli archivi annuali per ritrovarli qui.'
  ))
  for (const file of files) {
    const row = element('label', 'file')
    if (selectable) {
      const checkbox = document.createElement('input')
      checkbox.type = 'checkbox'
      checkbox.value = file.name
      checkbox.checked = selected.has(file.name)
      checkbox.disabled = state.task.running || busy
      checkbox.setAttribute('aria-label', 'Importa ' + file.name)
      checkbox.addEventListener('change', updateInputSelection)
      row.append(checkbox)
    }
    row.append(element('span', 'file-name', file.name), element('span', 'size', bytes(file.size)))
    root.append(row)
  }
}
function setBusy() {
  const locked = state.task.running || busy || !state.configured
  document.querySelectorAll('[data-mutation], [data-operation]').forEach(control => {
    control.disabled = locked || control.dataset.kept === 'true'
  })
  document.querySelectorAll('[data-gallery-view]').forEach(button => { button.disabled = locked })
  document.querySelectorAll('[data-view]').forEach(button => { button.disabled = busy || (!state.configured && button.dataset.view !== 'settings') })
  document.querySelectorAll('[data-settings]').forEach(button => { button.disabled = busy || state.task.running })
  $('save-settings').disabled = busy || state.task.running || Object.values(folders).some(path => !path)
  $('cancel-task').disabled = !state.task.running || state.task.cancel_requested || cancelling
  $('input-list').querySelectorAll('input').forEach(input => { input.disabled = locked })
  $('previous').disabled = page <= 1 || locked
  $('next').disabled = page >= pages || locked
  $('browse-previous').disabled = browsePage <= 1 || locked
  $('browse-next').disabled = browsePage >= browsePages || locked
  $('trash-previous').disabled = trashPage <= 1 || locked
  $('trash-next').disabled = trashPage >= trashPages || locked
  document.querySelectorAll('[data-cleanup-mode]').forEach(button => { button.disabled = locked })
  $('cleanup-previous').disabled = cleanupPage <= 1 || locked
  $('cleanup-next').disabled = cleanupPage >= cleanupPages || locked
  updateCleanupSelection()
  updateInputSelection()
}
async function refreshStorage() {
  if (!state.configured) return
  const storage = await api('/api/storage')
  const cards = []
  for (const [key, label] of [['inputs', 'Inputs'], ['gallery', 'Gallery'], ['exports', 'Exports'], ['trash', 'Cestino']]) {
    const stat = storage[key]
    const card = element('div', 'storage-stat')
    card.append(element('span', 'muted', label), element('strong', '', bytes(stat.bytes)), element('span', 'stats-note', `${stat.files} file`))
    cards.push(card)
  }
  const disk = element('div', 'storage-stat')
  disk.append(element('span', 'muted', 'Disco libero'), element('strong', '', bytes(storage.disk.free)), element('span', 'stats-note', `su ${bytes(storage.disk.total)}`))
  cards.push(disk)
  $('storage').replaceChildren(...cards)
}
async function refresh() {
  const wasRunning = state.task.running
  state = await api('/api/state')
  $('status').textContent = state.task.running ? 'Operazione in corso' : (state.task.status || 'Pronto')
  $('status').dataset.running = String(state.task.running)
  $('status').dataset.error = String(state.task.status === 'Fallito')
  $('gallery-path').textContent = state.folders?.gallery || state.gallery || 'Non configurata'
  $('gallery-path').title = $('gallery-path').textContent
  $('storage-overview').hidden = !state.configured
  if (!state.configured) {
    view('settings')
  } else {
    renderFiles('input-list', state.inputs || [], true)
    renderFiles('export-list', state.exports || [], false)
  }
  renderProgress()
  const log = Array.isArray(state.task.log) ? state.task.log.join('\n') : state.task.log
  const logText = log || 'Il dettaglio comparirà qui quando avvierai un’operazione.'
  if ($('log').textContent !== logText) $('log').textContent = logText
  setBusy()
  if ((wasRunning || cleanupTask) && !state.task.running && state.configured) {
    browseLoaded = false
    galleryOptionsLoaded = false
    trashLoaded = false
    cleanupLoaded = false
    if (cleanupTask) await completeCleanupTask()
    await refreshStorage()
    if (currentView === 'gallery') await run(loadActiveGallery, true)
    if (currentView === 'cleanup' && !cleanupLoaded) await run(() => loadCleanup(cleanupPage), true)
  }
}
async function task(action) {
  const body = { action }
  if (action === 'import') {
    body.inputs = [...$('input-list').querySelectorAll('input:checked')].map(input => input.value)
    if (!body.inputs.length) throw new Error('Seleziona almeno uno ZIP da importare.')
    body.password = $('import-password').value
    body.workers = Number($('workers').value)
  }
  if (action === 'export') {
    body.password = $('export-password').value
    body.password_confirmation = $('export-password-confirmation').value
    if (body.password !== body.password_confirmation) throw new Error('Le password di esportazione non coincidono.')
  }
  await api('/api/tasks', body)
  cleanupTask = action === 'analyze' ? { kind: 'analyze' } : null
  $('cleanup-return').hidden = action !== 'analyze'
  $('cleanup-task-result').hidden = true
  $('cleanup-task-result').replaceChildren()
  if (action === 'import') $('import-password').value = ''
  if (action === 'export') {
    $('export-password').value = ''
    $('export-password-confirmation').value = ''
  }
  view('tasks')
  await refresh()
}
const previewQueue = []
let activePreviews = 0
const lazyMedia = new IntersectionObserver(entries => {
  for (const entry of entries) {
    if (!entry.isIntersecting) continue
    lazyMedia.unobserve(entry.target)
    previewQueue.push(entry.target)
  }
  drainPreviews()
}, { rootMargin: '200px' })
async function drainPreviews() {
  while (activePreviews < 3 && previewQueue.length) {
    const media = previewQueue.shift()
    if (!media.isConnected) continue
    activePreviews += 1
    native.invoke('media_source', { path: media.dataset.path })
      .then(result => {
        if (media.isConnected) media.src = native.convertFileSrc(result.path)
      })
      .catch(() => previewError(media))
      .finally(() => { activePreviews -= 1; drainPreviews() })
  }
}
function previewError(media) {
  if (!media.isConnected) return
  media.parentElement.replaceChildren(element('span', 'muted', 'Anteprima non disponibile per questo formato.'))
}
function mediaPreview(item) {
  const preview = element('div', 'preview')
  const video = item.kind === 'videos'
  const media = document.createElement(video ? 'video' : 'img')
  if (video) { media.controls = true; media.preload = 'metadata' } else { media.alt = item.path; media.loading = 'lazy' }
  media.dataset.path = item.path
  media.addEventListener('error', () => previewError(media))
  media.addEventListener(video ? 'loadedmetadata' : 'load', () => { preview.dataset.loaded = 'true' })
  preview.append(media)
  lazyMedia.observe(media)
  return preview
}
function clearPreviews(root) {
  root.querySelectorAll('img, video').forEach(media => {
    lazyMedia.unobserve(media)
    if (media.tagName === 'VIDEO') media.pause()
    media.removeAttribute('src')
  })
  root.replaceChildren()
}
function dateInfo(label, value) {
  const block = element('div', 'date-info')
  block.append(element('span', 'muted', label), element('strong', '', value || 'Non disponibile'))
  return block
}
async function saveDate(item, date, keepGoogle = false) {
  if (!keepGoogle && !date) throw new Error('Scegli una data valida.')
  cleanupLoaded = false
  await api(keepGoogle ? '/api/keep' : '/api/correct', keepGoogle ? { path: item.path } : { path: item.path, date })
  browseLoaded = false
  await loadConflicts(page)
  await refreshStorage()
}
function renderConflict(item) {
  const card = element('article', 'card review')
  const details = document.createElement('div')
  const dates = element('div', 'dates')
  dates.append(dateInfo('Nome file', item.filename_date), dateInfo('Google · UTC', item.google_date), dateInfo('EXIF', item.exif_date), dateInfo('Data attuale', item.current_date))
  const picker = document.createElement('input')
  picker.type = 'date'
  picker.dataset.operation = ''
  picker.setAttribute('aria-label', 'Data da impostare per ' + item.path)
  picker.value = item.filename_date || ''
  const quick = element('div', 'quick')
  for (const [label, value] of [['Nome file', item.filename_date], ['Google', item.google_date], ['EXIF', item.exif_date], ['Attuale', item.current_date]]) {
    if (!value) continue
    quick.append(actionButton(label, async () => { picker.value = value }))
  }
  const actions = element('div', 'row')
  actions.append(actionButton('Imposta data', () => saveDate(item, picker.value), 'primary'), actionButton('Conferma data Google', () => saveDate(item, null, true)))
  details.append(element('h2', '', item.path.split('/').pop()), element('p', 'path muted', item.path), dates, picker, quick, actions)
  card.append(mediaPreview(item), details)
  return card
}
async function loadConflicts(target = 1, refresh = false) {
  return withLoading('conflicts', async () => {
    $('conflict-count').textContent = 'Scansione in corso…'
    const result = await api('/api/conflicts?page=' + target + (refresh ? '&refresh=1' : ''))
    page = result.page
    pages = result.pages
    $('conflict-count').textContent = `${result.total} file da verificare`
    clearPreviews($('conflicts'))
    $('conflicts').append(...result.items.map(renderConflict))
    if (!result.items.length) $('conflicts').append(emptyState('Nessuna discrepanza rilevata', 'Nessuna discrepanza da verificare tra i nomi riconosciuti e le date Google disponibili.'))
    $('pager').hidden = pages <= 1
    $('page-label').textContent = `${page} / ${pages}`
  })
}
async function galleryAction(action, item) {
  await api('/api/gallery/' + action, { path: item.path })
  cleanupLoaded = false
  trashLoaded = false
  await loadGallery(browsePage)
  await refreshStorage()
}
function renderGalleryItem(item) {
  const card = element('article', 'card gallery-card')
  const actions = element('div', 'row')
  const keep = actionButton(item.kept ? 'Già tenuta' : 'Tieni', () => galleryAction('keep', item), item.kept ? 'kept-badge' : 'primary')
  keep.dataset.kept = String(item.kept)
  keep.disabled = item.kept
  actions.append(keep, actionButton('Sposta nel cestino', () => galleryAction('trash', item)))
  card.append(mediaPreview(item), element('p', 'path', item.path), element('span', 'muted', bytes(item.size)), actions)
  return card
}
async function loadGalleryOptions() {
  const options = await api('/api/gallery/options')
  for (const [id, values, placeholder] of [['year-filter', options.years, 'Tutti gli anni'], ['month-filter', options.months, 'Tutti i mesi']]) {
    const select = $(id)
    const current = select.value
    select.replaceChildren()
    const all = element('option', '', placeholder)
    all.value = ''
    select.append(all)
    for (const value of values) {
      const option = element('option', '', value === 'Unknown year' ? 'Anno sconosciuto' : value === 'Unknown month' ? 'Mese sconosciuto' : value)
      option.value = value
      select.append(option)
    }
    select.value = values.includes(current) ? current : ''
  }
  galleryOptionsLoaded = true
}
async function loadGallery(target = 1, refresh = false) {
  return withLoading('gallery-items', async () => {
    $('browse-count').textContent = 'Caricamento gallery…'
    if (refresh) {
      await api('/api/gallery?page=1&refresh=1')
      galleryOptionsLoaded = false
    }
    if (!galleryOptionsLoaded) await loadGalleryOptions()
    const query = new URLSearchParams({ page: target, review: $('review-filter').value, year: $('year-filter').value, month: $('month-filter').value })
    const result = await api('/api/gallery?' + query)
    browsePage = result.page
    browsePages = result.pages
    browseLoaded = true
    $('browse-count').textContent = `${result.total} file · ${$('review-filter').value === 'pending' ? 'Da rivedere' : 'Tutta la gallery'}`
    clearPreviews($('gallery-items'))
    $('gallery-items').append(...result.items.map(renderGalleryItem))
    if (!result.items.length) $('gallery-items').append(emptyState('Nessun file in questa vista', $('review-filter').value === 'pending' ? 'Hai rivisto tutti i file visibili. Scegli Tutti i file o cambia anno e mese per continuare.' : 'Prova a cambiare i filtri o importa nuovi ZIP nella gallery.'))
    $('browse-pager').hidden = browsePages <= 1
    $('browse-page-label').textContent = `${browsePage} / ${browsePages}`
  })
}
async function restore(item) {
  await api('/api/gallery/restore', { id: item.id })
  cleanupLoaded = false
  browseLoaded = false
  await loadTrash(trashPage)
  await refreshStorage()
}
async function loadTrash(target = 1) {
  return withLoading('trash-items', async () => {
    const result = await api('/api/trash?page=' + target)
    trashPage = result.page
    trashPages = result.pages
    trashLoaded = true
    $('trash-count').textContent = `${result.total} file nel cestino. Restano sul disco e possono essere ripristinati.`
    $('trash-items').replaceChildren()
    for (const item of result.items) {
      const row = element('div', 'file trash-row')
      row.append(element('span', 'file-name path', item.path), element('span', 'size', bytes(item.size)), actionButton('Ripristina', () => restore(item)))
      $('trash-items').append(row)
    }
    if (!result.items.length) $('trash-items').append(emptyState('Cestino vuoto', 'I file spostati qui resteranno sul disco e potranno essere ripristinati.'))
    $('trash-pager').hidden = trashPages <= 1
    $('trash-page-label').textContent = `${trashPage} / ${trashPages}`
  })
}
async function loadActiveGallery() {
  if (galleryView === 'browse' && !browseLoaded) await loadGallery()
  if (galleryView === 'trash' && !trashLoaded) await loadTrash()
}
function galleryTab(name) {
  galleryView = name
  for (const key of ['browse', 'dates', 'trash']) $(key + '-pane').hidden = key !== name
  document.querySelectorAll('[data-gallery-view]').forEach(button => {
    const active = button.dataset.galleryView === name
    button.classList.toggle('active', active)
    button.setAttribute('aria-selected', String(active))
    button.tabIndex = active ? 0 : -1
  })
  if (state.configured && !state.task.running) run(loadActiveGallery, true)
}
async function run(operation, exclusive = false) {
  if (exclusive && (busy || state.task.running)) return
  clearError()
  if (exclusive) { busy = true; setBusy() }
  try { await operation() } catch (error) { showError(error) } finally {
    if (exclusive) { busy = false; setBusy() }
  }
}
function renderProgress() {
  const task = state.task
  const done = Number(task.done || 0)
  const total = Number(task.total || 0)
  if (total > 0) {
    $('task-progress').max = total
    $('task-progress').value = Math.min(done, total)
    $('progress-label').textContent = `${done} / ${total}${task.cancel_requested ? ' · Annullamento richiesto' : ''}`
  } else {
    $('task-progress').removeAttribute('value')
    $('progress-label').textContent = task.running ? (task.cancel_requested ? 'Annullamento richiesto…' : 'Operazione in corso…') : 'Nessuna attività in corso.'
    if (!task.running) { $('task-progress').max = 1; $('task-progress').value = 0 }
  }
  $('cancel-task').textContent = task.cancel_requested ? 'Interruzione richiesta' : 'Interrompi attività'
  $('task-status').textContent = task.running ? (task.cancel_requested ? 'Interruzione in corso…' : 'Operazione in corso') : (task.status === 'Pronto' || !task.status ? 'Nessuna attività in corso' : task.status)
}
function renderFolders() {
  $('folder-choices').replaceChildren()
  for (const [kind, title, hint] of [
    ['inputs', 'Inputs', 'Cartella con gli ZIP originali da importare.'],
    ['gallery', 'Gallery', 'Cartella per foto, video, metadati e decisioni di revisione.'],
    ['exports', 'Exports', 'Cartella in cui salvare gli ZIP annuali.']
  ]) {
    const choice = element('div', 'folder-choice')
    const row = element('div', 'row')
    const button = element('button', '', folders[kind] ? 'Cambia cartella' : 'Scegli cartella')
    button.dataset.settings = ''
    button.addEventListener('click', () => run(async () => {
      const selected = await native.invoke('pick_folder', { kind })
      if (selected) { folders[kind] = selected; renderFolders() }
    }, true))
    row.append(element('span', 'path', folders[kind] || 'Nessuna cartella selezionata'), button)
    choice.append(element('h3', '', title), element('p', 'muted', hint), row)
    $('folder-choices').append(choice)
  }
  $('settings-heading').textContent = state.configured ? 'Cartelle del tuo archivio' : 'Prima di iniziare, scegli tre cartelle'
  const changed = state.configured && Object.keys(folders).some(key => folders[key] !== state.folders?.[key])
  $('settings-summary').textContent = state.configured ? (changed ? 'Modifiche non salvate. Salva per applicare i nuovi percorsi.' : 'Cartelle salvate. Saranno riaperte al prossimo avvio.') : 'Scegli tutte e tre le cartelle per iniziare.'
  setBusy()
}
async function saveSettings() {
  if (Object.values(folders).some(path => !path)) throw new Error('Scegli tutte e tre le cartelle.')
  await api('/api/settings', folders)
  cleanupSelection.clear()
  cleanupLoaded = false
  clearPreviews($('cleanup-items'))
  $('cleanup-result').hidden = true
  browseLoaded = false
  trashLoaded = false
  galleryOptionsLoaded = false
  clearPreviews($('gallery-items'))
  clearPreviews($('conflicts'))
  $('trash-items').replaceChildren()
  await refresh()
  renderFolders()
  await refreshStorage()
  view('inputs')
  notify('Cartelle salvate. Puoi iniziare a gestire il tuo archivio.')
}
function notify(message) {
  $('notice').textContent = message
  $('notice').hidden = false
  $('notice-box').hidden = false
}
async function saveReport() {
  const result = await native.invoke('save_report')
  const path = typeof result === 'string' ? result : result?.path
  if (path) notify('Rapporto CSV salvato: ' + path)
}
async function cancelTask() {
  if (cancelling || !state.task.running || state.task.cancel_requested) return
  cancelling = true
  setBusy()
  try { await api('/api/cancel', {}); await refresh() } finally { cancelling = false; setBusy() }
}
async function initialize() {
  const settings = await api('/api/settings')
  folders = { inputs: '', gallery: '', exports: '', ...(settings.folders || {}) }
  await refresh()
  renderFolders()
  if (state.configured) { await refreshStorage(); view('inputs') }
  else view('settings')
}
const cleanupSelection = new Map()
let cleanupMode = 'large'
let cleanupPage = 1
let cleanupPages = 1
let cleanupLoaded = false
let cleanupTask = null
let cleanupGroups = []
function updateCleanupSelection() {
  const count = cleanupSelection.size
  const size = [...cleanupSelection.values()].reduce((sum, item) => sum + item.size, 0)
  $('cleanup-busy').hidden = !state.task.running
  $('cleanup-selection-hint').textContent = state.task.running
    ? 'Selezione e cestino bloccati fino al termine dell’attività. Segui l’avanzamento in Attività.'
    : 'Seleziona tu i file · massimo 500 · dimensione logica indicativa'
  $('cleanup-selection-count').textContent = count ? `${count} file ${count === 1 ? 'selezionato' : 'selezionati'} · ${bytes(size)}` : 'Nessun file selezionato'
  $('cleanup-clear').disabled = !count || busy || state.task.running || !state.configured
  $('cleanup-trash').disabled = !count || count > 500 || busy || state.task.running || !state.configured
}
function selectCleanupItem(item, checked) {
  if (checked && !item.fingerprint) throw new Error('Identità del file non disponibile. Aggiorna i suggerimenti prima di selezionarlo.')
  if (checked && !cleanupSelection.has(item.path) && cleanupSelection.size >= 500) {
    throw new Error('Puoi selezionare al massimo 500 file per operazione. Riduci la selezione o completa prima questo gruppo.')
  }
  if (checked) cleanupSelection.set(item.path, { path: item.path, fingerprint: item.fingerprint, size: Number(item.size || 0) })
  else cleanupSelection.delete(item.path)
  updateCleanupSelection()
}
function renderCleanupItem(item) {
  const card = element('article', 'card gallery-card')
  const label = element('label', 'cleanup-choice')
  const checkbox = document.createElement('input')
  checkbox.type = 'checkbox'
  checkbox.dataset.operation = ''
  checkbox.dataset.cleanupPath = item.path
  checkbox.setAttribute('aria-describedby', 'cleanup-selection-hint')
  checkbox.checked = cleanupSelection.has(item.path)
  checkbox.setAttribute('aria-label', 'Seleziona ' + item.path)
  checkbox.addEventListener('change', () => {
    clearError()
    try { selectCleanupItem(item, checkbox.checked) } catch (error) { checkbox.checked = false; showError(error) }
  })
  label.append(checkbox, element('span', '', 'Seleziona per il cestino'))
  card.append(mediaPreview(item), element('p', 'path', item.path), element('span', 'muted', bytes(item.size)))
  if (item.score !== undefined && item.score !== null) {
    const label = cleanupMode === 'similar' ? 'Distanza dal riferimento' : 'Dettaglio indicativo'
    card.append(element('p', 'muted', `${label}: ${Number(item.score).toFixed(1)}`))
  }
  card.append(label)
  return card
}
function selectCleanupGroup(group) {
  const additions = group.items.filter(item => !cleanupSelection.has(item.path))
  if (cleanupSelection.size + additions.length > 500) throw new Error('Il gruppo supera il limite di 500 file selezionati. Seleziona i file singolarmente o svuota la selezione.')
  for (const item of group.items) selectCleanupItem(item, true)
  $('cleanup-items').querySelectorAll('input[data-cleanup-path]').forEach(input => { input.checked = cleanupSelection.has(input.dataset.cleanupPath) })
}
function renderCleanupGroup(group, index) {
  const section = element('article', 'cleanup-group')
  const heading = element('div', 'cleanup-group-heading')
  const text = document.createElement('div')
  const labels = { large: 'File grande', bursts: 'Scatti ravvicinati', similar: 'Foto visivamente vicine', quality: 'Dettaglio da verificare' }
  const reasons = { large: 'Supera la dimensione minima', burst: 'Scatti entro l’intervallo scelto', similar: 'Aspetto simile al riferimento', quality: 'Dettaglio stimato sotto la soglia' }
  text.append(element('h3', '', `${labels[cleanupMode]} · ${index + 1 + (cleanupPage - 1) * 24}`), element('p', 'muted', `${reasons[group.reason] || 'Suggerimento da verificare'} · ${group.items.length} file · ${bytes(group.size || 0)}`))
  heading.append(text, actionButton('Seleziona gruppo', async () => selectCleanupGroup(group)))
  const items = element('div', 'gallery-grid')
  items.append(...group.items.map(renderCleanupItem))
  section.append(heading, items)
  return section
}
function renderCleanupSummary(summary) {
  $('cleanup-summary').replaceChildren()
  for (const [key, label] of [['eligible', 'Candidati'], ['analyzed', 'Analizzati'], ['pending', 'Da analizzare'], ['unsupported', 'Non supportati'], ['kept_excluded', 'Protetti da Tieni'], ['groups', 'Gruppi']]) {
    const item = document.createElement('span')
    item.append(element('strong', '', String(summary[key] || 0)), document.createTextNode(' ' + label))
    $('cleanup-summary').append(item)
  }
}
async function loadCleanup(target = 1, refresh = false) {
  if (!state.configured) return
  const query = new URLSearchParams({
    mode: cleanupMode, page: target,
    min_bytes: Number($('cleanup-min-size').value) * 1024 * 1024,
    gap_seconds: $('cleanup-gap').value,
    distance: $('cleanup-distance').value,
    quality_threshold: $('cleanup-quality').value
  })
  if (refresh) query.set('refresh', '1')
  $('cleanup-items').setAttribute('aria-busy', 'true')
  try {
    const result = await api('/api/cleanup?' + query)
    cleanupPage = result.page
    cleanupPages = result.pages
    cleanupGroups = result.items
    cleanupLoaded = true
    renderCleanupSummary(result.summary)
    clearPreviews($('cleanup-items'))
    $('cleanup-items').append(...result.items.map(renderCleanupGroup))
    if (!result.items.length) {
      const requiresAnalysis = ['similar', 'quality'].includes(cleanupMode) && result.summary.pending > 0
      const message = requiresAnalysis ? 'Analizza le foto per completare i suggerimenti. Alcuni file attendono ancora l’analisi locale.' : result.summary.eligible === 0 ? 'Nessun file idoneo da proporre. I file protetti con Tieni restano esclusi.' : 'Nessun suggerimento con questi filtri. Prova una soglia diversa.'
      $('cleanup-items').append(element('div', 'card empty', message))
    }
    $('cleanup-pager').hidden = cleanupPages <= 1
    $('cleanup-page-label').textContent = `${cleanupPage} / ${cleanupPages}`
  } finally {
    $('cleanup-items').setAttribute('aria-busy', 'false')
    setBusy()
  }
}
function cleanupTab(mode) {
  cleanupMode = mode
  cleanupLoaded = false
  $('cleanup-items').setAttribute('aria-labelledby', 'cleanup-tab-' + mode)
  for (const key of ['large', 'bursts', 'similar', 'quality']) $('cleanup-' + key + '-filter').hidden = key !== mode
  document.querySelectorAll('[data-cleanup-mode]').forEach(button => {
    const active = button.dataset.cleanupMode === mode
    button.classList.toggle('active', active)
    button.setAttribute('aria-selected', String(active))
    button.tabIndex = active ? 0 : -1
  })
  run(() => loadCleanup(1), true)
}
async function cleanupTrash() {
  if (!cleanupSelection.size) throw new Error('Seleziona almeno un file.')
  if (cleanupSelection.size > 500) throw new Error('Puoi spostare al massimo 500 file per operazione.')
  const items = [...cleanupSelection.values()].map(({ path, fingerprint }) => ({ path, fingerprint }))
  const result = await api('/api/cleanup/trash', { items })
  if (!result.started) throw new Error('Il task di pulizia non è stato avviato.')
  cleanupTask = { kind: 'trash' }
  $('cleanup-return').hidden = false
  $('cleanup-task-result').hidden = true
  view('tasks')
  await refresh()
}
function showCleanupOutcome(result) {
  const moved = result.moved || []
  const failed = result.failed || []
  for (const item of moved) cleanupSelection.delete(item.path)
  for (const id of ['cleanup-result', 'cleanup-task-result']) {
    const root = $(id)
    root.replaceChildren()
    root.hidden = false
    const card = element('div', 'cleanup-outcome' + (failed.length || result.cancelled ? ' has-failures' : ''))
    card.append(element('strong', '', `${moved.length} file ${moved.length === 1 ? 'spostato' : 'spostati'} nel cestino${failed.length ? ` · ${failed.length} non ${failed.length === 1 ? 'spostato' : 'spostati'}` : ''}`))
    if (result.cancelled) card.append(element('p', '', 'Operazione interrotta. I file già spostati restano nel cestino; gli altri rimangono nella gallery.'))
    if (failed.length) {
      const list = document.createElement('ul')
      for (const item of failed) list.append(element('li', '', `${item.path}: ${item.error}`))
      card.append(list)
    }
    card.append(element('p', 'muted', 'I file nel cestino possono essere ripristinati dalla Gallery. Nessuna eliminazione definitiva.'))
    root.append(card)
  }
  updateCleanupSelection()
}
async function completeCleanupTask() {
  const completed = cleanupTask
  cleanupTask = null
  if (completed?.kind === 'trash') {
    if (state.task.result) showCleanupOutcome(state.task.result)
    else {
      showError(new Error('Task terminato senza un riepilogo disponibile. Consulta il log e aggiorna i suggerimenti prima di riprovare.'))
      cleanupSelection.clear()
    }
  } else if (completed?.kind === 'analyze') {
    notify(`Analisi: ${state.task.status}. Torna alla pulizia per controllare i suggerimenti; consulta il log per eventuali file non analizzati.`)
  }
  cleanupLoaded = false
  await loadCleanup(cleanupPage, true)
}

document.querySelectorAll('[data-view]').forEach(button => button.addEventListener('click', () => view(button.dataset.view)))
document.querySelectorAll('[data-gallery-view]').forEach(button => button.addEventListener('click', () => galleryTab(button.dataset.galleryView)))
$('dismiss-error').addEventListener('click', clearError)
$('dismiss-notice').addEventListener('click', () => { $('notice-box').hidden = true; $('notice').hidden = true })
$('select-inputs').addEventListener('click', () => selectInputs(true))
$('clear-inputs').addEventListener('click', () => selectInputs(false))
enableTabKeyboard('[data-gallery-view]')
enableTabKeyboard('[data-cleanup-mode]')
$('refresh').addEventListener('click', () => run(refresh, true))
$('save-settings').addEventListener('click', () => run(saveSettings, true))
$('save-report').addEventListener('click', () => run(saveReport, true))
$('cancel-task').addEventListener('click', () => run(cancelTask))
$('refresh-storage').addEventListener('click', () => run(refreshStorage, true))
$('import').addEventListener('click', () => run(() => task('import'), true))
$('export').addEventListener('click', () => run(() => task('export'), true))
$('scan').addEventListener('click', () => run(() => loadConflicts(1, true), true))
$('previous').addEventListener('click', () => run(() => loadConflicts(page - 1), true))
$('next').addEventListener('click', () => run(() => loadConflicts(page + 1), true))
$('refresh-gallery').addEventListener('click', () => run(() => loadGallery(browsePage, true), true))
for (const id of ['review-filter', 'year-filter', 'month-filter']) $(id).addEventListener('change', () => run(() => loadGallery(1), true))
$('browse-previous').addEventListener('click', () => run(() => loadGallery(browsePage - 1), true))
$('browse-next').addEventListener('click', () => run(() => loadGallery(browsePage + 1), true))
$('refresh-trash').addEventListener('click', () => run(() => loadTrash(trashPage), true))
$('trash-previous').addEventListener('click', () => run(() => loadTrash(trashPage - 1), true))
$('trash-next').addEventListener('click', () => run(() => loadTrash(trashPage + 1), true))
document.querySelectorAll('[data-cleanup-mode]').forEach(button => button.addEventListener('click', () => cleanupTab(button.dataset.cleanupMode)))
$('cleanup-analyze').addEventListener('click', () => run(() => task('analyze'), true))
$('cleanup-refresh').addEventListener('click', () => run(() => loadCleanup(cleanupPage, true), true))
$('cleanup-previous').addEventListener('click', () => run(() => loadCleanup(cleanupPage - 1), true))
$('cleanup-next').addEventListener('click', () => run(() => loadCleanup(cleanupPage + 1), true))
for (const id of ['cleanup-min-size', 'cleanup-gap', 'cleanup-distance', 'cleanup-quality']) $(id).addEventListener('change', () => run(() => loadCleanup(1), true))
$('cleanup-clear').addEventListener('click', () => run(async () => { cleanupSelection.clear(); $('cleanup-items').querySelectorAll('input[data-cleanup-path]').forEach(input => { input.checked = false }); updateCleanupSelection() }, true))
$('cleanup-trash').addEventListener('click', () => run(cleanupTrash, true))
$('cleanup-return').addEventListener('click', () => view('cleanup'))
$('cleanup-show-task').addEventListener('click', () => view('tasks'))
run(initialize, true)
setInterval(async () => {
  if (polling || busy) return
  polling = true
  try { await refresh() } catch (error) { showError(error) } finally { polling = false }
}, 1000)
