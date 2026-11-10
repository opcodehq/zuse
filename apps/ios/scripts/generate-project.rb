# Run from any directory with Ruby and the xcodeproj gem installed.
require 'xcodeproj'
require 'json'
require 'fileutils'
require 'open3'
root = File.expand_path('..', __dir__)
profile = JSON.parse(File.read(File.join(root, '../../packages/contracts/src/deployment-profiles.json')))['production']
project = Xcodeproj::Project.new(File.join(root, 'Zuse.xcodeproj'))
target = project.new_target(:application, 'Zuse', :ios, '18.0')
target.product_name = 'Zuse Native'
target.product_reference.path = 'Zuse Native.app'
group = project.main_group.new_group('App', 'App')
Dir.glob(File.join(root, 'App/*.swift')).sort.each do |file|
  target.source_build_phase.add_file_reference(group.new_file(File.basename(file)))
end
FileUtils.cp(File.join(root, '../web/public/models/v1.json'), File.join(root, 'App/ModelCatalog.json')) unless ARGV.include?('--use-bundled-resources')
target.resources_build_phase.add_file_reference(group.new_file('ModelCatalog.json'))
unless ARGV.include?('--use-bundled-resources')
  abort 'Could not generate native settings schema.' unless system('bun', File.join(root, 'scripts/settings-schema.ts'))
end
target.resources_build_phase.add_file_reference(group.new_file('SettingsSchema.json'))
unless ARGV.include?('--use-bundled-resources')
unless system('bun', 'build', File.join(root, 'scripts/ui-parser.ts'), '--target=browser', '--format=iife', '--outfile=' + File.join(root, 'App/UIParser.js'))
  abort 'Could not generate the shared UI parser.'
end
end
target.resources_build_phase.add_file_reference(group.new_file('UIParser.js'))
# Reuse the canonical native icon without introducing a second design asset.
mac_icon = File.join(root, 'App/Assets.xcassets/MacAppIcon.appiconset')
FileUtils.mkdir_p(mac_icon)
FileUtils.cp(File.join(root, 'App/Assets.xcassets/AppIcon.appiconset/icon.png'), File.join(mac_icon, 'icon.png'))
target.resources_build_phase.add_file_reference(group.new_file('Assets.xcassets'))
unless ARGV.include?('--use-bundled-resources')
mermaid_entry, mermaid_status = Open3.capture2('bun', '-e', 'console.log(Bun.resolveSync("mermaid", process.argv[1]))', File.join(root, '../renderer'))
abort 'Install workspace dependencies before generating diagram resources.' unless mermaid_status.success?
mermaid_dist = File.dirname(mermaid_entry.strip)
FileUtils.cp(File.join(mermaid_dist, 'mermaid.min.js'), File.join(root, 'App/Mermaid.js'))
FileUtils.cp(File.join(mermaid_dist, '../LICENSE'), File.join(root, 'App/Mermaid-LICENSE.txt'))
end
['Diagram.html', 'Mermaid.js', 'Mermaid-LICENSE.txt', 'ThirdPartyNotices.txt'].each do |resource|
  target.resources_build_phase.add_file_reference(group.new_file(resource))
end
core = project.new(Xcodeproj::Project::Object::XCLocalSwiftPackageReference)
core.relative_path = '.'
project.root_object.package_references << core
textual = project.new(Xcodeproj::Project::Object::XCRemoteSwiftPackageReference)
textual.repositoryURL = 'https://github.com/gonzalezreal/textual.git'
textual.requirement = { 'kind' => 'exactVersion', 'version' => '0.5.0' }
project.root_object.package_references << textual
[['ZuseCore', core], ['Textual', textual]].each do |name, package|
  product = project.new(Xcodeproj::Project::Object::XCSwiftPackageProductDependency)
  product.product_name = name
  product.package = package
  target.package_product_dependencies << product
  build = project.new(Xcodeproj::Project::Object::PBXBuildFile)
  build.product_ref = product
  target.frameworks_build_phase.files << build
end
target.build_configurations.each do |config|
  config.build_settings.merge!({
    'PRODUCT_BUNDLE_IDENTIFIER' => 'sh.zuse.native',
    'PRODUCT_NAME' => 'Zuse Native',
    'ASSETCATALOG_COMPILER_APPICON_NAME' => 'AppIcon',
    'SWIFT_VERSION' => '6.0',
    'TARGETED_DEVICE_FAMILY' => '1,2',
    'SUPPORTS_MACCATALYST' => 'NO',
    'SUPPORTS_MAC_DESIGNED_FOR_IPHONE_IPAD' => 'NO',
    'GENERATE_INFOPLIST_FILE' => 'NO',
    'INFOPLIST_FILE' => 'App/Info.plist',
    'CODE_SIGN_STYLE' => 'Automatic',
    'CURRENT_PROJECT_VERSION' => '1',
    'MARKETING_VERSION' => '0.1.0'
  })
end
Xcodeproj::Plist.write_to_path({
  'CFBundleDisplayName' => 'Zuse Native',
  'CFBundleIdentifier' => '$(PRODUCT_BUNDLE_IDENTIFIER)',
  'CFBundleExecutable' => '$(EXECUTABLE_NAME)',
  'CFBundleName' => '$(PRODUCT_NAME)',
  'CFBundlePackageType' => 'APPL',
  'CFBundleShortVersionString' => '$(MARKETING_VERSION)',
  'CFBundleVersion' => '$(CURRENT_PROJECT_VERSION)',
  'LSRequiresIPhoneOS' => true,
  'UILaunchScreen' => {},
  'NSCameraUsageDescription' => 'Scan a pairing QR code to connect to your computer.',
  'NSLocalNetworkUsageDescription' => 'Find and connect to your Zuse computer on the local network.',
  'NSBonjourServices' => ['_zuse._tcp'],
  'NSAppTransportSecurity' => { 'NSAllowsLocalNetworking' => true },
  'UIApplicationSceneManifest' => { 'UIApplicationSupportsMultipleScenes' => false },
  'UISupportedInterfaceOrientations' => ['UIInterfaceOrientationPortrait', 'UIInterfaceOrientationLandscapeLeft', 'UIInterfaceOrientationLandscapeRight'],
  'UISupportedInterfaceOrientations~ipad' => ['UIInterfaceOrientationPortrait', 'UIInterfaceOrientationPortraitUpsideDown', 'UIInterfaceOrientationLandscapeLeft', 'UIInterfaceOrientationLandscapeRight'],
  'WorkOSClientID' => profile['workosPublicClientId'],
  'ZuseAPIURL' => profile['apiUrl']
}, File.join(root, 'App/Info.plist'))
ui_tests = project.new_target(:ui_test_bundle, 'ZuseUITests', :ios, '18.0')
ui_tests.add_dependency(target)
test_group = project.main_group.new_group('UITests', 'UITests')
Dir.glob(File.join(root, 'UITests/*.swift')).sort.each do |file|
  ui_tests.source_build_phase.add_file_reference(test_group.new_file(File.basename(file)))
end
ui_tests.build_configurations.each do |config|
  config.build_settings.merge!({ 'PRODUCT_BUNDLE_IDENTIFIER' => 'sh.zuse.native.uitests', 'SWIFT_VERSION' => '6.0', 'GENERATE_INFOPLIST_FILE' => 'YES', 'TEST_TARGET_NAME' => 'Zuse', 'TARGETED_DEVICE_FAMILY' => '1,2' })
end
project.save
scheme = Xcodeproj::XCScheme.new
scheme.add_build_target(target)
scheme.set_launch_target(target)
scheme.add_test_target(ui_tests)
scheme.save_as(project.path, 'Zuse', true)

# The Mac app shares the protocol, persistence and SwiftUI presentation with iOS.
# Keep platform integration behind NativePlatform rather than forking app behavior.
mac = project.new_target(:application, 'ZuseMac', :osx, '15.0')
mac.product_name = 'Zuse Native'
mac.product_reference.path = 'Zuse Native.app'
target.source_build_phase.files.each { |file| mac.source_build_phase.add_file_reference(file.file_ref) }
target.resources_build_phase.files.each { |file| mac.resources_build_phase.add_file_reference(file.file_ref) }
[['ZuseCore', core], ['Textual', textual]].each do |name, package|
  product = project.new(Xcodeproj::Project::Object::XCSwiftPackageProductDependency)
  product.product_name = name
  product.package = package
  mac.package_product_dependencies << product
  build = project.new(Xcodeproj::Project::Object::PBXBuildFile)
  build.product_ref = product
  mac.frameworks_build_phase.files << build
end
mac.build_configurations.each do |config|
  config.build_settings.merge!({
    'PRODUCT_BUNDLE_IDENTIFIER' => 'sh.zuse.native.mac',
    'ASSETCATALOG_COMPILER_APPICON_NAME' => 'MacAppIcon',
    'PRODUCT_NAME' => 'Zuse Native',
    'SWIFT_VERSION' => '6.0',
    'GENERATE_INFOPLIST_FILE' => 'NO',
    'INFOPLIST_FILE' => 'App/MacInfo.plist',
    'CODE_SIGN_STYLE' => 'Automatic',
    'ENABLE_HARDENED_RUNTIME' => 'YES',
    'CURRENT_PROJECT_VERSION' => '1',
    'MARKETING_VERSION' => '0.1.0',
    'COMBINE_HIDPI_IMAGES' => 'YES'
  })
end
mac_info = Xcodeproj::Plist.read_from_path(File.join(root, 'App/Info.plist'))
mac_info.delete_if { |key, _| key.start_with?('UI') || key == 'LSRequiresIPhoneOS' }
mac_info['NSPrincipalClass'] = 'NSApplication'
mac_info['LSMinimumSystemVersion'] = '$(MACOSX_DEPLOYMENT_TARGET)'
Xcodeproj::Plist.write_to_path(mac_info, File.join(root, 'App/MacInfo.plist'))
project.save
mac_tests = project.new_target(:ui_test_bundle, 'ZuseMacUITests', :osx, '15.0')
mac_tests.add_dependency(mac)
mac_test_group = project.main_group.new_group('MacUITests', 'MacUITests')
Dir.glob(File.join(root, 'MacUITests/*.swift')).sort.each do |file|
  mac_tests.source_build_phase.add_file_reference(mac_test_group.new_file(File.basename(file)))
end
mac_tests.build_configurations.each do |config|
  config.build_settings.merge!({
    'PRODUCT_BUNDLE_IDENTIFIER' => 'sh.zuse.native.mac.uitests',
    'SWIFT_VERSION' => '6.0',
    'GENERATE_INFOPLIST_FILE' => 'YES',
    'TEST_TARGET_NAME' => 'ZuseMac'
  })
end
project.save
mac_scheme = Xcodeproj::XCScheme.new
mac_scheme.add_build_target(mac)
mac_scheme.set_launch_target(mac)
mac_scheme.add_test_target(mac_tests)
mac_scheme.save_as(project.path, 'ZuseMac', true)
