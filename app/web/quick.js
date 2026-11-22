// The Quick add / reply panel (a local page with the app's IPC; the Board page itself never gets IPC).
// Add: one line to the Board's Inbox.
// Reply: the card's question and a text answer, sent through the signed device route.
(() => {
  const invoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args)
  const $ = id => document.getElementById(id)
  const form = $('form'), title = $('title'), reply = $('reply'), status = $('status'), keys = $('keys'), openBoard = $('open-board')
  let mode = 'add', busy = false, closeTimer = 0

  function say(text, kind = '') {
    status.textContent = text
    status.className = `status ${kind}`
    keys.hidden = Boolean(text)
  }

  async function refresh() {
    clearTimeout(closeTimer)
    const info = await invoke('panel_info').catch(() => ({ mode: 'add', prefill: '' }))
    mode = info.mode === 'reply' ? 'reply' : 'add'
    busy = false
    delete form.dataset.busy
    openBoard.hidden = true
    say('')
    $('context').hidden = mode !== 'reply'
    title.hidden = mode === 'reply'
    reply.hidden = mode !== 'reply'
    if (mode === 'reply') {
      $('ask').textContent = info.title || 'Reply'
      $('question').textContent = info.text && info.text !== info.title ? info.text : ''
      reply.value = ''
      keys.innerHTML = '<kbd>⌘</kbd><kbd>↩</kbd> Send <span class="sep">·</span> <kbd>esc</kbd> Close'
      reply.focus()
    } else {
      title.value = info.prefill || ''
      keys.innerHTML = '<kbd>↩</kbd> Add to Inbox <span class="sep">·</span> <kbd>esc</kbd> Close'
      title.focus()
      title.select()
    }
  }

  async function submit() {
    if (busy) return
    const text = (mode === 'reply' ? reply.value : title.value).trim()
    if (!text) return say(mode === 'reply' ? 'Write an answer first.' : 'Type what to add.', 'error')
    busy = true
    form.dataset.busy = ''
    say(mode === 'reply' ? 'Sending…' : 'Adding…')
    try {
      const done = await invoke('panel_submit', { text, note: '' })
      say(`${done} ✓`, 'ok')
      closeTimer = setTimeout(() => invoke('panel_hide'), 650)
    } catch (error) {
      busy = false
      delete form.dataset.busy
      say(String(error), 'error')
      openBoard.hidden = !/sign|Board window/i.test(String(error))
    }
  }

  form.addEventListener('submit', e => { e.preventDefault(); submit() })
  title.addEventListener('keydown', e => { if (e.key === 'Enter' && !e.isComposing) { e.preventDefault(); submit() } })
  reply.addEventListener('keydown', e => { if (e.key === 'Enter' && e.metaKey) { e.preventDefault(); submit() } })
  document.addEventListener('keydown', e => { if (e.key === 'Escape') { e.preventDefault(); invoke('panel_hide') } })
  openBoard.addEventListener('click', () => invoke('panel_open_board'))
  window.addEventListener('focus', () => { if (!busy) refresh() })
  window.boardPanel = { refresh }
  refresh()
})()
