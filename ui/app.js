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
  exports: ['Esportazione ZIP', 'Prepara una copia annuale del tuo archivio.'],
  tasks: ['Attività', 'Segui le operazioni in corso e consulta il risultato.'],
  settings: ['Impostazioni', 'Scegli dove conservare i tuoi archivi.']
}
function showError(error) {
  $('error').textContent = error?.message || String(error)
  $('error').hidden = false
}
function clearError() { $('error').hidden = true }
function view(name) {
  if (!state.configured && name !== 'settings') name = 'settings'
  currentView = name
  for (const key of Object.keys(titles)) $(key).hidden = key !== name
  document.querySelectorAll('[data-view]').forEach(button => button.classList.toggle('active', button.dataset.view === name))
  $('title').textContent = titles[name][0]
  $('subtitle').textContent = titles[name][1]
  if (name === 'gallery' && state.configured && !state.task.running) run(loadActiveGallery, true)
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
  if (text !== undefined) node.textContent = text
  return node
}
function actionButton(label, operation, className = '') {
  const button = element('button', className, label)
  button.dataset.mutation = ''
  button.addEventListener('click', () => run(operation, true))
  return button
}
function renderFiles(id, files, selectable) {
  const root = $(id)
  const signature = JSON.stringify(files)
  if (root.dataset.signature === signature) return
  root.dataset.signature = signature
  const selected = new Set([...root.querySelectorAll('input:checked')].map(input => input.value))
  root.replaceChildren()
  if (!files.length) root.append(element('div', 'empty', 'Nessun archivio disponibile.'))
  for (const file of files) {
    const row = element('label', 'file')
    if (selectable) {
      const checkbox = document.createElement('input')
      checkbox.type = 'checkbox'
      checkbox.value = file.name
      checkbox.checked = selected.has(file.name)
      checkbox.disabled = state.task.running || busy
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
  $('status').textContent = state.task.running ? '● Operazione in corso' : (state.task.status || 'Pronto')
  $('gallery-path').textContent = state.folders?.gallery || state.gallery || 'Non configurata'
  $('storage-overview').hidden = !state.configured
  if (!state.configured) {
    view('settings')
  } else {
    renderFiles('input-list', state.inputs || [], true)
    renderFiles('export-list', state.exports || [], false)
  }
  renderProgress()
  const log = Array.isArray(state.task.log) ? state.task.log.join('\n') : state.task.log
  $('log').textContent = log || 'Nessuna attività.'
  setBusy()
  if (wasRunning && !state.task.running && state.configured) {
    browseLoaded = false
    galleryOptionsLoaded = false
    trashLoaded = false
    await refreshStorage()
    if (currentView === 'gallery') await run(loadActiveGallery, true)
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
  $('conflict-count').textContent = 'Scansione in corso…'
  const result = await api('/api/conflicts?page=' + target + (refresh ? '&refresh=1' : ''))
  page = result.page
  pages = result.pages
  $('conflict-count').textContent = `${result.total} file da verificare`
  clearPreviews($('conflicts'))
  $('conflicts').append(...result.items.map(renderConflict))
  if (!result.items.length) $('conflicts').append(element('div', 'card empty', 'Nessuna discrepanza da verificare.'))
  $('pager').hidden = pages <= 1
  $('page-label').textContent = `${page} / ${pages}`
}
async function galleryAction(action, item) {
  await api('/api/gallery/' + action, { path: item.path })
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
  if (!result.items.length) $('gallery-items').append(element('div', 'card empty', 'Nessun file in questa vista.'))
  $('browse-pager').hidden = browsePages <= 1
  $('browse-page-label').textContent = `${browsePage} / ${browsePages}`
}
async function restore(item) {
  await api('/api/gallery/restore', { id: item.id })
  browseLoaded = false
  await loadTrash(trashPage)
  await refreshStorage()
}
async function loadTrash(target = 1) {
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
  if (!result.items.length) $('trash-items').append(element('div', 'empty', 'Cestino vuoto.'))
  $('trash-pager').hidden = trashPages <= 1
  $('trash-page-label').textContent = `${trashPage} / ${trashPages}`
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
  $('cancel-task').textContent = task.cancel_requested ? 'Annullamento richiesto' : 'Annulla attività'
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
  $('settings-summary').textContent = state.configured ? 'Le modifiche saranno applicate dopo il salvataggio.' : 'Seleziona tutte e tre le cartelle per iniziare.'
  setBusy()
}
async function saveSettings() {
  if (Object.values(folders).some(path => !path)) throw new Error('Scegli tutte e tre le cartelle.')
  await api('/api/settings', folders)
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
document.querySelectorAll('[data-view]').forEach(button => button.addEventListener('click', () => view(button.dataset.view)))
document.querySelectorAll('[data-gallery-view]').forEach(button => button.addEventListener('click', () => galleryTab(button.dataset.galleryView)))
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
run(initialize, true)
setInterval(async () => {
  if (polling || busy) return
  polling = true
  try { await refresh() } catch (error) { showError(error) } finally { polling = false }
}, 1000)
