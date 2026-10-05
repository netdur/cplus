param([long]$WindowHandle, [int]$Count)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$window = [System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$WindowHandle)
function Find-Name([string]$Name) {
    $condition = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty, $Name)
    return $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition)
}
$deadline = (Get-Date).AddSeconds(8)
do {
    $label = Find-Name "Facet clicked $Count times"
    $button = Find-Name 'Increment'
    $heading = Find-Name 'Facet, rendered by WinUI'
    if ($label -and $button -and $heading -and !$button.Current.IsOffscreen -and $button.Current.BoundingRectangle.Width -gt 0) { break }
    Start-Sleep -Milliseconds 100
} while ((Get-Date) -lt $deadline)
if (!$label -or !$button -or !$heading) { throw "Native UI did not show count $Count" }
if (!$button.Current.IsEnabled) { throw 'Button is disabled' }
$bounds = $button.Current.BoundingRectangle
$editCondition = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Edit)
$edit = $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $editCondition)
if (!$edit) { throw 'TextBox missing' }
$editBounds = $edit.Current.BoundingRectangle
$editValue = $edit.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value
$scrollCondition = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::IsScrollPatternAvailableProperty, $true)
$scrollers = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $scrollCondition)
$scroll = $null
foreach ($candidate in $scrollers) {
    $pattern = $candidate.GetCurrentPattern([System.Windows.Automation.ScrollPattern]::Pattern)
    if ($pattern.Current.VerticallyScrollable) { $scroll = $candidate; $scrollState = $pattern.Current; break }
}
if (!$scroll) { throw 'No vertically scrollable native content' }
$scrollBounds = $scroll.Current.BoundingRectangle
@{ button = @($bounds.X,$bounds.Y,$bounds.Width,$bounds.Height); headingWidth = $heading.Current.BoundingRectangle.Width
   edit = @($editBounds.X,$editBounds.Y,$editBounds.Width,$editBounds.Height); text = $editValue
   scroll = @($scrollBounds.X,$scrollBounds.Y,$scrollBounds.Width,$scrollBounds.Height); scrollPercent = $scrollState.VerticalScrollPercent
   echo = [bool](Find-Name 'Typed: WinUI') } | ConvertTo-Json -Compress
