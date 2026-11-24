import { Resource } from '@opencloud-eu/web-client'
import {
  ApplicationInformation,
  AppWrapperRoute,
  defineWebApplication
} from '@opencloud-eu/web-pkg'
import translations from '../l10n/translations.json'
import App from './App.vue'
import { useGettext } from 'vue3-gettext'

const applicationId = 'officecraft'

// What OfficeCraft opens (kind_for in officecraft/src/main.rs). The first of each group is
// offered in the "New" menu.
const WORD = ['docx', 'docm', 'dotx', 'odt', 'rtf']
const SHEET = ['xlsx', 'xlsm']
const SLIDES = ['pptx', 'potx', 'ppsx']
const PDF = ['pdf']

export default defineWebApplication({
  setup() {
    const { $gettext } = useGettext()

    const routes = [
      {
        name: 'officecraft',
        path: '/:driveAliasAndItem(.*)?',
        component: AppWrapperRoute(App, {
          applicationId,
          // Office files are binary: hand the editor an ArrayBuffer, not text.
          fileContentOptions: { responseType: 'arraybuffer' },
          // OfficeCraft saves when the user presses Save; nothing to autosave in between.
          disableAutoSave: true
        }),
        meta: {
          authContext: 'hybrid',
          patchCleanPath: true
        }
      }
    ]

    const newMenu: Record<string, () => string> = {
      docx: () => $gettext('OfficeCraft document'),
      xlsx: () => $gettext('OfficeCraft spreadsheet'),
      pptx: () => $gettext('OfficeCraft presentation')
    }

    const appInfo: ApplicationInformation = {
      name: 'OfficeCraft',
      id: applicationId,
      icon: 'file-text',
      color: '#E8722C',
      defaultExtension: 'docx',
      extensions: [...WORD, ...SHEET, ...SLIDES, ...PDF].map((extension) => ({
        extension,
        routeName: 'officecraft',
        ...(newMenu[extension] ? { newFileMenu: { menuTitle: newMenu[extension] } } : {})
      }))
    }

    return {
      appInfo,
      routes,
      translations
    }
  }
})
