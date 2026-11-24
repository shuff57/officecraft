<template>
  <div class="officecraft-app">
    <iframe
      ref="frame"
      class="officecraft-frame"
      :src="src"
      :title="$gettext('OfficeCraft')"
      allow="clipboard-read; clipboard-write; fullscreen"
    />
  </div>
</template>

<script setup lang="ts">
// Hosts OfficeCraft's browser build in an iframe on this same origin and talks to it through the
// bridge in officecraft/src/host.rs (`?host=parent`):
//   officecraft:ready  -> we send officecraft:open { name, bytes }
//   officecraft:save   { name, saveAs, bytes } -> the opened file (same name) is saved through
//                      AppWrapper (ETag conflict handling); any other name (Export as PDF,
//                      Save As) is written next to it in the same folder
//   officecraft:dirty  { dirty } -> guards leaving the page
//   officecraft:failed { error } -> shown as an error message
import { computed, nextTick, onBeforeUnmount, onMounted, PropType, ref, unref } from 'vue'
import { Resource, SpaceResource } from '@opencloud-eu/web-client'
import { AppConfigObject, useClientService, useMessages } from '@opencloud-eu/web-pkg'
import { useGettext } from 'vue3-gettext'

const props = defineProps({
  resource: { type: Object as PropType<Resource>, required: true },
  space: { type: Object as PropType<SpaceResource>, required: true },
  applicationConfig: {
    type: Object as PropType<AppConfigObject>,
    required: true,
    default: (): AppConfigObject => undefined
  },
  currentContent: { type: [ArrayBuffer, String] as PropType<ArrayBuffer | string>, required: true },
  isReadOnly: { type: Boolean, required: true },
  isDirty: { type: Boolean, required: true }
})
const emit = defineEmits(['update:currentContent', 'save', 'close'])

const { $gettext } = useGettext()
const { showMessage, showErrorMessage } = useMessages()
const clientService = useClientService()

const frame = ref<HTMLIFrameElement>()
const editorDirty = ref(false)
// A file created empty from the "New" menu: OfficeCraft names a new document itself, so its first
// save with the same extension belongs to this file whatever name it carries.
const createdEmpty = ref(false)

const src = computed(() => {
  const base = (props.applicationConfig?.url as string) || '/craft/officecraft/'
  const url = new URL(base, window.location.origin)
  url.searchParams.set('host', 'parent')
  return url.pathname + url.search
})

const ext = (name: string) => (name.includes('.') ? name.split('.').pop().toLowerCase() : '')

function toBytes(content: ArrayBuffer | string): ArrayBuffer {
  if (content instanceof ArrayBuffer) return content
  if (!content) return new ArrayBuffer(0)
  return new TextEncoder().encode(content).buffer as ArrayBuffer
}

function post(message: Record<string, unknown>) {
  unref(frame)?.contentWindow?.postMessage(message, window.location.origin)
}

function sendFile() {
  const bytes = toBytes(props.currentContent)
  createdEmpty.value = bytes.byteLength === 0
  // Structured clone copies the buffer; AppWrapper keeps its own as the saved state.
  post({ type: 'officecraft:open', name: props.resource.name, bytes })
}

async function saveCopy(name: string, bytes: ArrayBuffer) {
  const folder = props.resource.path.split('/').slice(0, -1).join('/')
  const path = `${folder}/${name}`
  await clientService.webdav.putFileContents(props.space, { path, content: bytes })
  showMessage({ title: $gettext('Saved %{name} next to %{file}', { name, file: props.resource.name }) })
}

async function onSave(name: string, bytes: ArrayBuffer) {
  if (props.isReadOnly) {
    showErrorMessage({ title: $gettext('This file is read-only'), errors: [] })
    return
  }
  const own = props.resource.name
  const sameFile = name === own || (unref(createdEmpty) && ext(name) === ext(own))
  try {
    if (sameFile) {
      createdEmpty.value = false
      emit('update:currentContent', bytes)
      await nextTick()
      emit('save')
    } else {
      await saveCopy(name, bytes)
    }
  } catch (e) {
    console.error(e)
    showErrorMessage({ title: $gettext('Could not save %{name}', { name }), errors: [e as Error] })
  }
}

async function onMessage(event: MessageEvent) {
  if (event.origin !== window.location.origin || event.source !== unref(frame)?.contentWindow) return
  const m = event.data
  if (!m || typeof m.type !== 'string' || !m.type.startsWith('officecraft:')) return
  switch (m.type) {
    case 'officecraft:ready':
      sendFile()
      break
    case 'officecraft:save':
      await onSave(String(m.name || props.resource.name), m.bytes as ArrayBuffer)
      break
    case 'officecraft:dirty':
      editorDirty.value = !!m.dirty
      break
    case 'officecraft:failed':
      showErrorMessage({ title: $gettext('OfficeCraft could not start'), errors: [new Error(m.error || '')] })
      break
  }
}

function guardUnload(e: BeforeUnloadEvent) {
  if (unref(editorDirty)) {
    e.preventDefault()
    e.returnValue = ''
  }
}

onMounted(() => {
  window.addEventListener('message', onMessage)
  window.addEventListener('beforeunload', guardUnload)
})
onBeforeUnmount(() => {
  window.removeEventListener('message', onMessage)
  window.removeEventListener('beforeunload', guardUnload)
})
</script>

<style scoped>
.officecraft-app {
  width: 100%;
  height: 100%;
  display: flex;
}
.officecraft-frame {
  flex: 1;
  width: 100%;
  height: 100%;
  border: 0;
}
</style>
