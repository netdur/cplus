# UI Automation for the WinUI e2e checks: facet keys are AutomationIds.
#   find   <hwnd> <id>   prints 1 when an element with that id is in the window, else 0
#   invoke <hwnd> <id>   invokes it (Invoke pattern; no focus or activation change)
#   name   <hwnd> <id>   prints its Name (a label's text)
#   focus  <hwnd> <id>   gives it the keyboard, which brings its window to the front
# Run with: powershell.exe -NoProfile -ExecutionPolicy Bypass -File tools/winui_uia.ps1 ...
param([string]$Action, [long]$WindowHandle, [string]$Id)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$window = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$WindowHandle)
$condition = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::AutomationIdProperty, $Id)
$found = $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition)
switch ($Action) {
    'find' { if ($found) { '1' } else { '0' } }
    'invoke' {
        if (!$found) { throw "No element '$Id'" }
        $found.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
        'invoked'
    }
    'name' {
        if (!$found) { throw "No element '$Id'" }
        $found.Current.Name
    }
    'focus' {
        if (!$found) { throw "No element '$Id'" }
        $found.SetFocus(); 'focused'
    }
    default { throw "Unknown action '$Action'" }
}
