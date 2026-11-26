# Regenerates Zuse.xcodeproj, App/Info.plist and App/MacInfo.plist.
# Requires Ruby with the xcodeproj gem; run from any directory:
#   ruby apps/ios/scripts/generate-project.rb
# Object identifiers are derived from the project structure, so rerunning
# without source changes leaves the project file untouched.
require 'xcodeproj'
require 'json'
require 'fileutils'

ROOT = File.expand_path('..', __dir__)
PROJECT_PATH = File.join(ROOT, 'Zuse.xcodeproj')
PROFILE = JSON.parse(File.read(File.join(ROOT, 'config/deployment-profiles.json'))).fetch('production')
SWIFT_VERSION = '6.0'
MARKETING_VERSION = '0.1.0'
# Bundled as-is; see README.md for where each resource comes from.
RESOURCES = %w[
  ModelCatalog.json SettingsSchema.json UIParser.js Assets.xcassets
  Diagram.html Mermaid.js Mermaid-LICENSE.txt ThirdPartyNotices.txt
].freeze

project = Xcodeproj::Project.new(PROJECT_PATH)
app_group = project.main_group.new_group('App', 'App')
sources = Dir.glob(File.join(ROOT, 'App/*.swift')).sort.map { |path| app_group.new_file(File.basename(path)) }
resources = RESOURCES.map { |name| app_group.new_file(name) }

# ZuseCore is the local Swift package; Textual renders Markdown and Citadel
# provides SSH sign-in and port forwarding for computers added over SSH.
core = project.new(Xcodeproj::Project::Object::XCLocalSwiftPackageReference)
core.relative_path = '.'
textual = project.new(Xcodeproj::Project::Object::XCRemoteSwiftPackageReference)
textual.repositoryURL = 'https://github.com/gonzalezreal/textual.git'
textual.requirement = { 'kind' => 'exactVersion', 'version' => '0.5.0' }
citadel = project.new(Xcodeproj::Project::Object::XCRemoteSwiftPackageReference)
citadel.repositoryURL = 'https://github.com/orlandos-nl/Citadel.git'
citadel.requirement = { 'kind' => 'upToNextMinorVersion', 'minimumVersion' => '0.12.1' }
project.root_object.package_references.push(core, textual, citadel)
PACKAGE_PRODUCTS = [['ZuseCore', core], ['Textual', textual], ['Citadel', citadel]].freeze

def link_packages(project, target)
  PACKAGE_PRODUCTS.each do |name, package|
    product = project.new(Xcodeproj::Project::Object::XCSwiftPackageProductDependency)
    product.product_name = name
    product.package = package
    target.package_product_dependencies << product
    build = project.new(Xcodeproj::Project::Object::PBXBuildFile)
    build.product_ref = product
    target.frameworks_build_phase.files << build
  end
end

def configure(target, settings)
  target.build_configurations.each { |config| config.build_settings.merge!(settings) }
end

def app_target(project, name, platform, deployment, sources, resources, settings)
  target = project.new_target(:application, name, platform, deployment)
  target.product_name = 'Zuse Native'
  target.product_reference.path = 'Zuse Native.app'
  sources.each { |file| target.source_build_phase.add_file_reference(file) }
  resources.each { |file| target.resources_build_phase.add_file_reference(file) }
  link_packages(project, target)
  configure(target, {
    'PRODUCT_NAME' => 'Zuse Native',
    'SWIFT_VERSION' => SWIFT_VERSION,
    'GENERATE_INFOPLIST_FILE' => 'NO',
    'CODE_SIGN_STYLE' => 'Automatic',
    'CURRENT_PROJECT_VERSION' => '1',
    'MARKETING_VERSION' => MARKETING_VERSION
  }.merge(settings))
  target
end

def ui_test_target(project, name, platform, deployment, app, directory, settings)
  target = project.new_target(:ui_test_bundle, name, platform, deployment)
  target.add_dependency(app)
  group = project.main_group.new_group(directory, directory)
  Dir.glob(File.join(ROOT, directory, '*.swift')).sort.each do |path|
    target.source_build_phase.add_file_reference(group.new_file(File.basename(path)))
  end
  configure(target, {
    'SWIFT_VERSION' => SWIFT_VERSION,
    'GENERATE_INFOPLIST_FILE' => 'YES',
    'TEST_TARGET_NAME' => app.name
  }.merge(settings))
  target
end

# The Mac app shares the protocol, persistence and SwiftUI presentation with iOS.
# Keep platform integration behind NativePlatform rather than forking app behavior.
ios = app_target(project, 'Zuse', :ios, '18.0', sources, resources, {
  'PRODUCT_BUNDLE_IDENTIFIER' => 'sh.zuse.native',
  'INFOPLIST_FILE' => 'App/Info.plist',
  'ASSETCATALOG_COMPILER_APPICON_NAME' => 'AppIcon',
  'TARGETED_DEVICE_FAMILY' => '1,2',
  'SUPPORTS_MACCATALYST' => 'NO',
  'SUPPORTS_MAC_DESIGNED_FOR_IPHONE_IPAD' => 'NO'
})
mac = app_target(project, 'ZuseMac', :osx, '15.0', sources, resources, {
  'PRODUCT_BUNDLE_IDENTIFIER' => 'sh.zuse.native.mac',
  'INFOPLIST_FILE' => 'App/MacInfo.plist',
  'ASSETCATALOG_COMPILER_APPICON_NAME' => 'MacAppIcon',
  'ENABLE_HARDENED_RUNTIME' => 'YES',
  'COMBINE_HIDPI_IMAGES' => 'YES'
})
# Both apps build "Zuse Native.app"; a distinct display name keeps their
# product references from hashing to the same identifier.
mac.product_reference.name = 'Zuse Native (Mac).app'
ios_tests = ui_test_target(project, 'ZuseUITests', :ios, '18.0', ios, 'UITests', {
  'PRODUCT_BUNDLE_IDENTIFIER' => 'sh.zuse.native.uitests',
  'TARGETED_DEVICE_FAMILY' => '1,2'
})
mac_tests = ui_test_target(project, 'ZuseMacUITests', :osx, '15.0', mac, 'MacUITests', {
  'PRODUCT_BUNDLE_IDENTIFIER' => 'sh.zuse.native.mac.uitests'
})

# new_target links Foundation/Cocoa through SDK-versioned paths. Swift links
# system frameworks automatically, so drop them to keep the project SDK-neutral.
project.frameworks_group.recursive_children.grep(Xcodeproj::Project::Object::PBXFileReference).each(&:remove_from_project)
project.frameworks_group.remove_from_project

# Twice: target dependency proxies embed target identifiers as plain strings,
# so the second pass hashes them after the targets themselves are stable.
2.times { project.predictabilize_uuids }
project.save
[[ios, ios_tests], [mac, mac_tests]].each do |app, tests|
  scheme = Xcodeproj::XCScheme.new
  scheme.add_build_target(app)
  scheme.set_launch_target(app)
  scheme.add_test_target(tests)
  scheme.save_as(PROJECT_PATH, app.name, true)
end

# The Mac icon reuses the canonical app icon rather than a second design asset.
mac_icon = File.join(ROOT, 'App/Assets.xcassets/MacAppIcon.appiconset')
FileUtils.mkdir_p(mac_icon)
FileUtils.cp(File.join(ROOT, 'App/Assets.xcassets/AppIcon.appiconset/icon.png'), File.join(mac_icon, 'icon.png'))

ios_info = {
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
  'NSLocalNetworkUsageDescription' => 'Connect to your Zuse computer on the local network.',
  'NSAppTransportSecurity' => { 'NSAllowsLocalNetworking' => true },
  'UIApplicationSceneManifest' => { 'UIApplicationSupportsMultipleScenes' => false },
  'UISupportedInterfaceOrientations' => %w[
    UIInterfaceOrientationPortrait UIInterfaceOrientationLandscapeLeft UIInterfaceOrientationLandscapeRight
  ],
  'UISupportedInterfaceOrientations~ipad' => %w[
    UIInterfaceOrientationPortrait UIInterfaceOrientationPortraitUpsideDown
    UIInterfaceOrientationLandscapeLeft UIInterfaceOrientationLandscapeRight
  ],
  'WorkOSClientID' => PROFILE.fetch('workosPublicClientId'),
  'ZuseAPIURL' => PROFILE.fetch('apiUrl')
}
Xcodeproj::Plist.write_to_path(ios_info, File.join(ROOT, 'App/Info.plist'))
mac_info = ios_info.reject { |key, _| key.start_with?('UI') || key == 'LSRequiresIPhoneOS' }
mac_info['NSPrincipalClass'] = 'NSApplication'
mac_info['LSMinimumSystemVersion'] = '$(MACOSX_DEPLOYMENT_TARGET)'
Xcodeproj::Plist.write_to_path(mac_info, File.join(ROOT, 'App/MacInfo.plist'))
puts "Generated #{PROJECT_PATH}"
