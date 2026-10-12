# UI Automation for the WinUI e2e checks: facet keys are AutomationIds.
#   find    <hwnd> <id>   prints 1 when an element with that id is in the window, else 0
#   invoke  <hwnd> <id>   invokes it (Invoke pattern; no focus or activation change)
#   name    <hwnd> <id>   prints its Name (a label's text)
#   value   <hwnd> <id>   prints its Value (a text field's text)
#   enabled <hwnd> <id>   prints True or False
#   expand  <hwnd> <id>   opens it (ExpandCollapse pattern: a menu)
#   focus   <hwnd> <id>   gives it the keyboard, which brings its window to the front
# An open menu's items are in a popup, so an id starting `menu:` that is not
# under the window is also looked for in the process's other top-level windows.
# Run with: powershell.exe -NoProfile -ExecutionPolicy Bypass -File tools/winui_uia.ps1 ...
param([string]$Action, [long]$WindowHandle, [string]$Id)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$AE = [System.Windows.Automation.AutomationElement]
$Scope = [System.Windows.Automation.TreeScope]
$window = $AE::FromHandle([IntPtr]$WindowHandle)
$condition = [System.Windows.Automation.PropertyCondition]::new($AE::AutomationIdProperty, $Id)
$found = $window.FindFirst($Scope::Descendants, $condition)
if (!$found -and $Id.StartsWith('menu:')) {
    $process = [System.Windows.Automation.PropertyCondition]::new($AE::ProcessIdProperty, $window.Current.ProcessId)
    foreach ($top in $AE::RootElement.FindAll($Scope::Children, $process)) {
        $found = $top.FindFirst($Scope::Descendants, $condition)
        if ($found) { break }
    }
}
if ($Action -ne 'find' -and !$found) { throw "No element '$Id'" }
switch ($Action) {
    'find' { if ($found) { '1' } else { '0' } }
    'invoke' { $found.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke(); 'invoked' }
    'name' { $found.Current.Name }
    'value' { $found.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value }
    'enabled' { [string]$found.Current.IsEnabled }
    'expand' { $found.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern).Expand(); 'expanded' }
    'focus' { $found.SetFocus(); 'focused' }
    default { throw "Unknown action '$Action'" }
}
