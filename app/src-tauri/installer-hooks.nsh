; 解除安裝時移除 native messaging host 的登錄機碼(安裝時由程式自己在每次啟動寫入)
!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegKey HKCU "Software\Google\Chrome\NativeMessagingHosts\com.agent_translator.host"
  DeleteRegKey HKCU "Software\Microsoft\Edge\NativeMessagingHosts\com.agent_translator.host"
  DeleteRegKey HKCU "Software\Chromium\NativeMessagingHosts\com.agent_translator.host"
!macroend
